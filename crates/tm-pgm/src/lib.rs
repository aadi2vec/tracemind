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

use chrono::Utc;
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
}

impl Variable {
    pub fn new(name: impl Into<String>, kind: VariableKind, domain: Vec<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            domain,
        }
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
    /// Create or update a variable.
    pub fn upsert_variable(conn: &Connection, v: &Variable) -> Result<()> {
        let domain = serde_json::to_string(&v.domain)
            .map_err(|e| TraceMindError::Storage(format!("domain: {e}")))?;
        conn.execute(
            "INSERT INTO lgm_variables (id, name, kind, domain, observed_at)
             VALUES (?,?,?,?,?)
             ON CONFLICT(name) DO UPDATE SET
               kind        = excluded.kind,
               domain      = excluded.domain,
               observed_at = excluded.observed_at",
            params![
                v.id.to_string(),
                v.name,
                v.kind.as_str(),
                domain,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert var: {e}")))?;
        Ok(())
    }

    pub fn list_variables(conn: &Connection) -> Result<Vec<Variable>> {
        let mut stmt = conn
            .prepare("SELECT id, name, kind, domain FROM lgm_variables ORDER BY name ASC")
            .map_err(|e| TraceMindError::Storage(format!("prep vars: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let kind: String = row.get(2)?;
                let domain: String = row.get(3)?;
                Ok((id, name, kind, domain))
            })
            .map_err(|e| TraceMindError::Storage(format!("vars query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, kind, domain) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(Variable {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                kind: VariableKind::parse(&kind),
                domain: serde_json::from_str(&domain).unwrap_or_default(),
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
        let r: std::result::Result<(String, String, String, String), _> = conn.query_row(
            "SELECT id, name, kind, domain FROM lgm_variables WHERE name = ?",
            params![name],
            |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            },
        );
        match r {
            Ok((id, name, kind, domain)) => Ok(Some(Variable {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                kind: VariableKind::parse(&kind),
                domain: serde_json::from_str(&domain).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE lgm_variables (
               id TEXT PRIMARY KEY, name TEXT UNIQUE,
               kind TEXT, domain TEXT, observed_at TEXT
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
