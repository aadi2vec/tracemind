//! tm-pgm — TraceMind Personal Graphical Model.
//!
//! Layer L2 of Sprint GRAPH. A *minimal, local-only, sparse-friendly*
//! probabilistic graphical model so that the system can reason about
//! plausible next-events (e.g. "Aaditya is likely working on Rondo
//! between 10am-noon based on yesterday + 2026-05-08 + 2026-05-11").
//!
//! Design choices:
//! - **Discrete categorical variables** (e.g. activity ∈ {rondo,
//!   tracemind, idle}; time_window ∈ {morning, afternoon, evening};
//!   context_id ∈ <ids>). Discrete = sparse-data friendly + fast EM.
//! - **Conditional probability tables (CPDs)** stored as JSON in
//!   `lgm_dependencies.cpd`. One row per (child, parent) pair.
//! - **Bayesian network**, not Markov random field. Acyclic so we can
//!   run belief propagation in topological order without a junction
//!   tree (sufficient for personal-scale graphs <100 variables).
//! - **Inference**: enumeration over parents, multiplicative scoring.
//! - **Learning**: simple Bayesian counting with Dirichlet prior α=1
//!   so cold-start variables don't produce NaN.
//!
//! Tables (`lgm_variables`, `lgm_dependencies`) live in `memory.db` —
//! schema is owned by `tm_graph::graph_sprint`.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    Activity,
    Context,
    TimeWindow,
    Mood,
    Topic,
    Outcome,
    Custom,
}

impl VariableKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VariableKind::Activity => "activity",
            VariableKind::Context => "context",
            VariableKind::TimeWindow => "time_window",
            VariableKind::Mood => "mood",
            VariableKind::Topic => "topic",
            VariableKind::Outcome => "outcome",
            VariableKind::Custom => "custom",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "activity" => VariableKind::Activity,
            "context" => VariableKind::Context,
            "time_window" => VariableKind::TimeWindow,
            "mood" => VariableKind::Mood,
            "topic" => VariableKind::Topic,
            "outcome" => VariableKind::Outcome,
            _ => VariableKind::Custom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub id: Uuid,
    pub name: String,
    pub kind: VariableKind,
    /// Discrete domain (values the variable can take).
    pub domain: Vec<String>,
    /// Optional Object Type from the ontology that this RV ranges over.
    /// `None` means the variable is free-form (legacy / `Custom`).
    /// `Some("Topic")` ties the variable's *meaning* to an Object Type so
    /// LGM-2's "Anticipate" surface can talk about "topic" rather than
    /// a raw string.
    #[serde(default)]
    pub object_type: Option<String>,
}

impl Variable {
    pub fn new(name: impl Into<String>, kind: VariableKind, domain: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            domain,
            object_type: None,
        }
    }

    pub fn with_object_type(mut self, ot: impl Into<String>) -> Self {
        self.object_type = Some(ot.into());
        self
    }
}

/// One row of a CPD: P(child=value | parents=joint_values).
///
/// Stored compactly: `parent_assignments` is a flat map
/// `parent_name -> parent_value`, and `probabilities` is `value -> p`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cpd {
    pub parent_assignments: BTreeMap<String, String>,
    pub probabilities: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub child_id: Uuid,
    pub parent_id: Uuid,
    /// CPT serialized as JSON; multiple rows -> array of `Cpd`.
    pub cpd: Vec<Cpd>,
    pub log_score: f64,
    pub support_count: u32,
}

/// One observation: a complete or partial assignment of variables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Observation {
    /// variable_name -> value
    pub values: HashMap<String, String>,
}

pub struct PgmStore;

impl PgmStore {
    /// Create or update a variable. Object Type column is optional and
    /// is *not* validated against the ontology here — callers using the
    /// `Variable::with_object_type` helper are responsible for naming a
    /// real Object Type. Validation is cheap to add later.
    pub fn upsert_variable(conn: &Connection, v: &Variable) -> Result<()> {
        let domain = serde_json::to_string(&v.domain)
            .map_err(|e| TraceMindError::Storage(format!("domain: {e}")))?;
        conn.execute(
            "INSERT INTO lgm_variables (id, name, kind, domain, observed_at, object_type)
             VALUES (?,?,?,?,?,?)
             ON CONFLICT(name) DO UPDATE SET
               kind        = excluded.kind,
               domain      = excluded.domain,
               object_type = excluded.object_type,
               observed_at = excluded.observed_at",
            params![
                v.id.to_string(),
                v.name,
                v.kind.as_str(),
                domain,
                Utc::now().to_rfc3339(),
                v.object_type,
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert var: {e}")))?;
        Ok(())
    }

    pub fn list_variables(conn: &Connection) -> Result<Vec<Variable>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, kind, domain, object_type
                 FROM lgm_variables ORDER BY name ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep vars: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let kind: String = row.get(2)?;
                let domain: String = row.get(3)?;
                let ot: Option<String> = row.get(4)?;
                Ok((id, name, kind, domain, ot))
            })
            .map_err(|e| TraceMindError::Storage(format!("vars query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, kind, domain, ot) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(Variable {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                kind: VariableKind::parse(&kind),
                domain: serde_json::from_str(&domain).unwrap_or_default(),
                object_type: ot,
            });
        }
        Ok(out)
    }

    pub fn upsert_dependency(conn: &Connection, dep: &Dependency) -> Result<()> {
        let cpd = serde_json::to_string(&dep.cpd)
            .map_err(|e| TraceMindError::Storage(format!("cpd: {e}")))?;
        conn.execute(
            "INSERT INTO lgm_dependencies
              (child_id, parent_id, cpd, log_score, support_count, updated_at)
             VALUES (?,?,?,?,?,?)
             ON CONFLICT(child_id, parent_id) DO UPDATE SET
               cpd           = excluded.cpd,
               log_score     = excluded.log_score,
               support_count = excluded.support_count,
               updated_at    = excluded.updated_at",
            params![
                dep.child_id.to_string(),
                dep.parent_id.to_string(),
                cpd,
                dep.log_score,
                dep.support_count as i64,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert dep: {e}")))?;
        Ok(())
    }

    pub fn parents_of(conn: &Connection, child: Uuid) -> Result<Vec<Dependency>> {
        let mut stmt = conn
            .prepare(
                "SELECT child_id, parent_id, cpd, log_score, support_count
                 FROM lgm_dependencies WHERE child_id = ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep par: {e}")))?;
        let rows = stmt
            .query_map(params![child.to_string()], |row| {
                let cid: String = row.get(0)?;
                let pid: String = row.get(1)?;
                let cpd: String = row.get(2)?;
                let log_score: f64 = row.get(3)?;
                let support: i64 = row.get(4)?;
                Ok((cid, pid, cpd, log_score, support))
            })
            .map_err(|e| TraceMindError::Storage(format!("par query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (cid, pid, cpd, log_score, support) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(Dependency {
                child_id: Uuid::parse_str(&cid)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                parent_id: Uuid::parse_str(&pid)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                cpd: serde_json::from_str(&cpd).unwrap_or_default(),
                log_score,
                support_count: support as u32,
            });
        }
        Ok(out)
    }

    pub fn get_variable_by_name(conn: &Connection, name: &str) -> Result<Option<Variable>> {
        let r: std::result::Result<
            (String, String, String, String, Option<String>),
            _,
        > = conn.query_row(
            "SELECT id, name, kind, domain, object_type
             FROM lgm_variables WHERE name = ?",
            params![name],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        );
        match r {
            Ok((id, name, kind, domain, ot)) => Ok(Some(Variable {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                kind: VariableKind::parse(&kind),
                domain: serde_json::from_str(&domain).unwrap_or_default(),
                object_type: ot,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(format!("var by name: {e}"))),
        }
    }
}

/// Online learning pass — for each observed (parent, child) we bump
/// the corresponding CPT row's count and re-normalize with a Dirichlet
/// α=1 prior so even single observations produce well-formed probabilities.
pub fn observe(conn: &Connection, obs: &Observation) -> Result<()> {
    let variables = PgmStore::list_variables(conn)?;
    let name_to_var: HashMap<&str, &Variable> =
        variables.iter().map(|v| (v.name.as_str(), v)).collect();

    for v in &variables {
        let Some(value) = obs.values.get(&v.name) else {
            continue;
        };
        let parents = PgmStore::parents_of(conn, v.id)?;
        if parents.is_empty() {
            // marginal — root prior. Stored as a single-row CPD against
            // a synthetic root id (Uuid::nil) so reads always work.
            let key = std::iter::empty::<(String, String)>().collect::<BTreeMap<_, _>>();
            let mut existing = PgmStore::parents_of(conn, v.id)?
                .into_iter()
                .find(|d| d.parent_id == Uuid::nil());
            let mut entry = existing
                .as_mut()
                .map(|d| d.cpd.clone())
                .unwrap_or_default();
            let row = entry
                .iter_mut()
                .find(|c| c.parent_assignments == key)
                .map(|c| &mut *c);
            match row {
                Some(c) => {
                    *c.probabilities.entry(value.clone()).or_insert(0.0) += 1.0;
                }
                None => {
                    let mut probs = BTreeMap::new();
                    probs.insert(value.clone(), 1.0);
                    entry.push(Cpd {
                        parent_assignments: key.clone(),
                        probabilities: probs,
                    });
                }
            }
            normalize(&mut entry, &v.domain);
            PgmStore::upsert_dependency(
                conn,
                &Dependency {
                    child_id: v.id,
                    parent_id: Uuid::nil(),
                    cpd: entry,
                    log_score: 0.0,
                    support_count: existing.as_ref().map(|d| d.support_count).unwrap_or(0) + 1,
                },
            )?;
        } else {
            // For each parent — store P(child|parent) marginalized as
            // separate per-parent CPDs (naive bayes-ish, keeps it small).
            for dep in parents {
                let parent_var = variables.iter().find(|v| v.id == dep.parent_id);
                let Some(parent_var) = parent_var else { continue };
                let Some(parent_value) = obs.values.get(&parent_var.name) else {
                    continue;
                };
                let mut entry = dep.cpd.clone();
                let key: BTreeMap<String, String> =
                    std::iter::once((parent_var.name.clone(), parent_value.clone())).collect();
                let row = entry
                    .iter_mut()
                    .find(|c| c.parent_assignments == key);
                match row {
                    Some(c) => {
                        *c.probabilities.entry(value.clone()).or_insert(0.0) += 1.0;
                    }
                    None => {
                        let mut probs = BTreeMap::new();
                        probs.insert(value.clone(), 1.0);
                        entry.push(Cpd {
                            parent_assignments: key.clone(),
                            probabilities: probs,
                        });
                    }
                }
                normalize(&mut entry, &v.domain);
                PgmStore::upsert_dependency(
                    conn,
                    &Dependency {
                        child_id: v.id,
                        parent_id: dep.parent_id,
                        cpd: entry,
                        log_score: 0.0,
                        support_count: dep.support_count + 1,
                    },
                )?;
            }
        }
    }
    let _ = name_to_var;
    Ok(())
}

/// Bucket a wall-clock timestamp into one of four named windows.
/// Cheap & deterministic; matches the `time_window` variable's domain
/// when seeded via [`ensure_baseline_variables`].
pub fn time_window_bucket(ts: DateTime<Utc>) -> &'static str {
    let h = ts.hour_local_naive();
    match h {
        5..=11 => "morning",
        12..=17 => "afternoon",
        18..=22 => "evening",
        _ => "night",
    }
}

/// Lightweight DateTime extension so we don't pull `chrono::Timelike`
/// into the public API; equivalent to `.hour() as u8`.
trait LocalHour {
    fn hour_local_naive(self) -> u8;
}
impl LocalHour for DateTime<Utc> {
    fn hour_local_naive(self) -> u8 {
        use chrono::Timelike;
        self.hour() as u8
    }
}

/// Make sure the baseline RVs that `observe_from_event_graph` writes
/// against exist. Idempotent — safe to call at LGM init.
///
/// Tied-to-ontology mapping:
///   - `event_kind`   → no ObjectType (it's a verb, not an entity)
///   - `time_window`  → no ObjectType (sub-attribute of an Event)
///   - `context_id`   → ObjectType: "Thread"
pub fn ensure_baseline_variables(conn: &Connection) -> Result<()> {
    let kind = Variable {
        id: Uuid::new_v4(),
        name: "event_kind".into(),
        kind: VariableKind::Activity,
        domain: vec![
            "capture".into(),
            "query".into(),
            "commitment".into(),
            "outcome".into(),
            "decision".into(),
        ],
        object_type: None,
    };
    let tw = Variable {
        id: Uuid::new_v4(),
        name: "time_window".into(),
        kind: VariableKind::TimeWindow,
        domain: vec![
            "morning".into(),
            "afternoon".into(),
            "evening".into(),
            "night".into(),
        ],
        object_type: None,
    };
    let ctx = Variable {
        id: Uuid::new_v4(),
        name: "context_id".into(),
        kind: VariableKind::Context,
        // Domain populated lazily as new contexts are observed; start empty.
        domain: vec![],
        object_type: Some("Thread".into()),
    };
    PgmStore::upsert_variable(conn, &kind)?;
    PgmStore::upsert_variable(conn, &tw)?;
    PgmStore::upsert_variable(conn, &ctx)?;
    Ok(())
}

/// Scan `event_nodes` in `[start, end]` and feed each event into the PGM
/// as an `Observation` over (event_kind, time_window, context_id). This
/// is how LGM "sees" the event graph — it is the inverse of the
/// LGM-2 *forecast* surface that reads marginals back out.
///
/// Returns the number of events ingested. Idempotent: re-running over
/// the same window deepens the prior — every event contributes one
/// Bayesian count. Callers that don't want double-counting should
/// move the window forward each run.
pub fn observe_from_event_graph(
    conn: &Connection,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<usize> {
    ensure_baseline_variables(conn)?;

    let start_ms = start.timestamp_millis();
    let end_ms = end.timestamp_millis();
    let mut stmt = conn
        .prepare(
            "SELECT kind, ts, context_id FROM event_nodes
             WHERE ts BETWEEN ? AND ? ORDER BY ts ASC",
        )
        .map_err(|e| TraceMindError::Storage(format!("prep ev scan: {e}")))?;
    let rows = stmt
        .query_map(params![start_ms, end_ms], |row| {
            let kind: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            let ctx: Option<String> = row.get(2)?;
            Ok((kind, ts, ctx))
        })
        .map_err(|e| TraceMindError::Storage(format!("ev scan: {e}")))?;

    // Materialize first so we can drop the borrow before calling observe()
    // (observe() reopens the conn for read).
    let mut buf: Vec<(String, i64, Option<String>)> = Vec::new();
    for r in rows {
        buf.push(r.map_err(|e| TraceMindError::Storage(e.to_string()))?);
    }
    drop(stmt);

    let mut count = 0usize;
    for (kind, ts_ms, ctx) in buf {
        let ts = Utc
            .timestamp_millis_opt(ts_ms)
            .single()
            .unwrap_or_else(Utc::now);
        let mut o = Observation::default();
        o.values.insert("event_kind".into(), kind);
        o.values
            .insert("time_window".into(), time_window_bucket(ts).into());
        if let Some(c) = ctx {
            // Grow context_id domain on first sight so observe() can score it.
            if let Some(mut var) = PgmStore::get_variable_by_name(conn, "context_id")? {
                if !var.domain.iter().any(|d| d == &c) {
                    var.domain.push(c.clone());
                    PgmStore::upsert_variable(conn, &var)?;
                }
            }
            o.values.insert("context_id".into(), c);
        }
        observe(conn, &o)?;
        count += 1;
    }
    Ok(count)
}

fn normalize(rows: &mut [Cpd], domain: &[String]) {
    for row in rows.iter_mut() {
        // Dirichlet α=1: every domain value gets +1 base mass.
        for v in domain {
            row.probabilities.entry(v.clone()).or_insert(0.0);
        }
        let sum: f64 = row.probabilities.values().sum::<f64>() + domain.len() as f64;
        if sum > 0.0 {
            for v in row.probabilities.values_mut() {
                *v = (*v + 1.0) / sum;
            }
        }
    }
}

/// Posterior over a single target variable, given evidence.
/// Combines marginals from each known parent independently
/// (naive-bayes style scoring). Returns sorted (value, prob) pairs.
pub fn posterior(
    conn: &Connection,
    target: &str,
    evidence: &Observation,
) -> Result<Vec<(String, f64)>> {
    let Some(var) = PgmStore::get_variable_by_name(conn, target)? else {
        return Ok(vec![]);
    };
    if var.domain.is_empty() {
        return Ok(vec![]);
    }
    let deps = PgmStore::parents_of(conn, var.id)?;
    let mut scores: HashMap<String, f64> =
        var.domain.iter().map(|d| (d.clone(), 1.0)).collect();
    let variables = PgmStore::list_variables(conn)?;

    let mut used_any = false;
    for dep in deps {
        if dep.parent_id == Uuid::nil() {
            // marginal prior
            if let Some(row) = dep.cpd.iter().find(|c| c.parent_assignments.is_empty()) {
                for d in &var.domain {
                    let p = row.probabilities.get(d).copied().unwrap_or(1e-6);
                    *scores.entry(d.clone()).or_insert(1.0) *= p;
                }
                used_any = true;
            }
            continue;
        }
        let Some(pvar) = variables.iter().find(|v| v.id == dep.parent_id) else {
            continue;
        };
        let Some(pval) = evidence.values.get(&pvar.name) else {
            continue;
        };
        let mut found = false;
        for row in &dep.cpd {
            if row
                .parent_assignments
                .get(&pvar.name)
                .map(|v| v == pval)
                .unwrap_or(false)
            {
                for d in &var.domain {
                    let p = row.probabilities.get(d).copied().unwrap_or(1e-6);
                    *scores.entry(d.clone()).or_insert(1.0) *= p;
                }
                found = true;
                used_any = true;
                break;
            }
        }
        let _ = found;
    }
    if !used_any {
        // No data at all — uniform.
        let n = var.domain.len() as f64;
        return Ok(var
            .domain
            .into_iter()
            .map(|d| (d, 1.0 / n))
            .collect());
    }
    let total: f64 = scores.values().sum();
    let mut out: Vec<(String, f64)> = scores
        .into_iter()
        .map(|(k, v)| (k, if total > 0.0 { v / total } else { 0.0 }))
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

// ── LGM-2 — Anticipate API ───────────────────────────────────────────────
//
// Top-level surface for the "what's likely next?" question. Returns a
// short, ranked list of `(value, prob, support)` rows so the Brief view
// can render lines like
//
//   "73% — event_kind=query (morning, ctx=acme-launch) — 12 obs"
//
// This is intentionally tiny — the heavy lifting is in `posterior()`.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnticipateRow {
    pub value: String,
    pub probability: f64,
    /// Total support behind the posterior (sum across parents).
    pub support: u32,
}

/// "Given this evidence, what's likely next?" — reads PGM marginals.
/// `target` is the variable name to predict (e.g. `"event_kind"`).
/// `evidence` is a map of `variable_name -> value` (e.g.
/// `{"time_window": "morning"}`). Returns the top `top_k` outcomes.
pub fn anticipate(
    conn: &Connection,
    target: &str,
    evidence: &Observation,
    top_k: usize,
) -> Result<Vec<AnticipateRow>> {
    let post = posterior(conn, target, evidence)?;
    let support: u32 = match PgmStore::get_variable_by_name(conn, target)? {
        Some(v) => PgmStore::parents_of(conn, v.id)?
            .into_iter()
            .map(|d| d.support_count)
            .sum(),
        None => 0,
    };
    let mut out: Vec<AnticipateRow> = post
        .into_iter()
        .map(|(value, probability)| AnticipateRow {
            value,
            probability,
            support,
        })
        .collect();
    if out.len() > top_k {
        out.truncate(top_k);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE lgm_variables (
               id TEXT PRIMARY KEY, name TEXT UNIQUE,
               kind TEXT, domain TEXT, observed_at TEXT,
               object_type TEXT
             );
             CREATE TABLE lgm_dependencies (
               child_id TEXT, parent_id TEXT, cpd TEXT,
               log_score REAL, support_count INTEGER, updated_at TEXT,
               PRIMARY KEY (child_id, parent_id)
             );",
        )
        .unwrap();
        c
    }

    #[test]
    fn upsert_round_trip() {
        let c = fresh();
        let v = Variable::new(
            "activity",
            VariableKind::Activity,
            vec!["rondo".into(), "tracemind".into(), "idle".into()],
        );
        PgmStore::upsert_variable(&c, &v).unwrap();
        let vs = PgmStore::list_variables(&c).unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].domain.len(), 3);
    }

    #[test]
    fn learn_marginal_from_observations() {
        let c = fresh();
        let v = Variable::new(
            "activity",
            VariableKind::Activity,
            vec!["rondo".into(), "tracemind".into()],
        );
        PgmStore::upsert_variable(&c, &v).unwrap();
        for _ in 0..3 {
            let mut o = Observation::default();
            o.values.insert("activity".into(), "tracemind".into());
            observe(&c, &o).unwrap();
        }
        let mut o = Observation::default();
        o.values.insert("activity".into(), "rondo".into());
        observe(&c, &o).unwrap();

        let post = posterior(&c, "activity", &Observation::default()).unwrap();
        let t = post.iter().find(|(k, _)| k == "tracemind").unwrap().1;
        let r = post.iter().find(|(k, _)| k == "rondo").unwrap().1;
        assert!(t > r);
    }

    #[test]
    fn variable_carries_object_type_round_trip() {
        let c = fresh();
        let v = Variable::new(
            "topic",
            VariableKind::Topic,
            vec!["rondo".into(), "tracemind".into()],
        )
        .with_object_type("Topic");
        PgmStore::upsert_variable(&c, &v).unwrap();
        let got = PgmStore::get_variable_by_name(&c, "topic").unwrap().unwrap();
        assert_eq!(got.object_type.as_deref(), Some("Topic"));
    }

    #[test]
    fn observe_from_event_graph_ingests_window() {
        let c = fresh();
        // Mimic the slice of event_nodes schema we read from.
        c.execute_batch(
            "CREATE TABLE event_nodes (
               id TEXT PRIMARY KEY, kind TEXT, ts INTEGER,
               payload_ref TEXT, cluster_id INTEGER,
               context_id TEXT, thread_id TEXT, salience REAL
             );",
        )
        .unwrap();

        let now = Utc::now();
        for off in &[0i64, -1000, -2000] {
            c.execute(
                "INSERT INTO event_nodes (id, kind, ts, payload_ref, salience)
                 VALUES (?, 'query', ?, '', 0.0)",
                params![Uuid::new_v4().to_string(), now.timestamp_millis() + *off],
            )
            .unwrap();
        }
        let count = observe_from_event_graph(
            &c,
            now - chrono::Duration::seconds(10),
            now + chrono::Duration::seconds(10),
        )
        .unwrap();
        assert_eq!(count, 3);

        // With only queries observed, the marginal should favor `query`
        // over every other event_kind in the domain.
        let ev = Observation::default();
        let rows = anticipate(&c, "event_kind", &ev, 5).unwrap();
        let q = rows.iter().find(|r| r.value == "query").unwrap().probability;
        for r in &rows {
            if r.value != "query" {
                assert!(q > r.probability, "query ({q}) should beat {} ({})", r.value, r.probability);
            }
        }
        // The Anticipate row should also carry a non-zero support count.
        assert!(rows.iter().any(|r| r.value == "query" && r.support >= 1));
    }

    #[test]
    fn posterior_conditioned_on_parent() {
        let c = fresh();
        let parent = Variable::new(
            "time_window",
            VariableKind::TimeWindow,
            vec!["morning".into(), "evening".into()],
        );
        let child = Variable::new(
            "activity",
            VariableKind::Activity,
            vec!["rondo".into(), "tracemind".into()],
        );
        PgmStore::upsert_variable(&c, &parent).unwrap();
        PgmStore::upsert_variable(&c, &child).unwrap();

        // Declare dependency by writing one stub row (so parents_of sees it).
        PgmStore::upsert_dependency(
            &c,
            &Dependency {
                child_id: child.id,
                parent_id: parent.id,
                cpd: vec![],
                log_score: 0.0,
                support_count: 0,
            },
        )
        .unwrap();

        // 5 mornings → rondo
        for _ in 0..5 {
            let mut o = Observation::default();
            o.values.insert("time_window".into(), "morning".into());
            o.values.insert("activity".into(), "rondo".into());
            observe(&c, &o).unwrap();
        }
        // 3 evenings → tracemind
        for _ in 0..3 {
            let mut o = Observation::default();
            o.values.insert("time_window".into(), "evening".into());
            o.values.insert("activity".into(), "tracemind".into());
            observe(&c, &o).unwrap();
        }

        let mut ev = Observation::default();
        ev.values.insert("time_window".into(), "morning".into());
        let post = posterior(&c, "activity", &ev).unwrap();
        let r = post.iter().find(|(k, _)| k == "rondo").unwrap().1;
        let t = post.iter().find(|(k, _)| k == "tracemind").unwrap().1;
        assert!(r > t, "morning should prefer rondo");
    }
}
