//! SQLite persistence for the system of intents.
//!
//! Tables (created on first open):
//!
//! - `commitments`        — one row per [`Commitment`]; heavy fields
//!                          (options_considered, derived_from, tags)
//!                          stored as JSON arrays
//! - `outcomes`           — one row per [`Outcome`]
//! - `anticipations`      — one row per [`Anticipation`]
//! - `context_snapshots`  — id + JSON blob (snapshots are heavy and we
//!                          rarely need the embedding loaded)
//!
//! We keep this file separate from `~/.tracemind/memory.db` (the
//! `tm-graph` SQLite store) because that DB's schema is managed by the
//! vendored `sqlite-knowledge-graph` crate and we don't want to fight
//! it. Default path is `~/.tracemind/intents.db`. The kg-side typed
//! edges (`Commitment ── about ──► Entity`, etc.) live in the graph
//! DB and reference commitment UUIDs from here.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    Anticipation, AnticipationKind, Commitment, CommitmentKind, ContextSnapshot, Outcome,
    OutcomeSource, Polarity, Source, Stakes, State, UserResponse,
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not found: {kind} {id}")]
    NotFound { kind: &'static str, id: Uuid },
    #[error("invalid {field}: {value}")]
    Invalid { field: &'static str, value: String },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Connection wrapper. Cheap to clone via `IntentStore::open` again
/// (each instance opens its own SQLite handle); not `Clone` itself
/// because `Connection` isn't.
pub struct IntentStore {
    conn: Connection,
}

impl IntentStore {
    /// Open (or create) the intent store at `path`. Pass `":memory:"`
    /// for an ephemeral DB (used by tests).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory store. Equivalent to `open(":memory:")`.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS commitments (
                id                   TEXT PRIMARY KEY,
                kind                 TEXT NOT NULL,
                statement            TEXT NOT NULL,
                made_at              TEXT NOT NULL,
                horizon              TEXT,
                context_snapshot_id  TEXT,
                options_considered   TEXT NOT NULL DEFAULT '[]',
                chosen               TEXT NOT NULL,
                confidence           REAL NOT NULL DEFAULT 0.7,
                expected_outcome     TEXT,
                stakes               TEXT NOT NULL,
                state                TEXT NOT NULL,
                outcome_id           TEXT,
                derived_from         TEXT NOT NULL DEFAULT '[]',
                tags                 TEXT NOT NULL DEFAULT '[]',
                source               TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_commitments_state    ON commitments(state);
            CREATE INDEX IF NOT EXISTS idx_commitments_horizon  ON commitments(horizon);
            CREATE INDEX IF NOT EXISTS idx_commitments_made_at  ON commitments(made_at);

            CREATE TABLE IF NOT EXISTS outcomes (
                id              TEXT PRIMARY KEY,
                commitment_id   TEXT NOT NULL,
                observed_at     TEXT NOT NULL,
                description     TEXT NOT NULL,
                polarity        TEXT NOT NULL,
                surprise        REAL NOT NULL DEFAULT 0.0,
                evidence_traces TEXT NOT NULL DEFAULT '[]',
                user_note       TEXT,
                source          TEXT NOT NULL,
                FOREIGN KEY(commitment_id) REFERENCES commitments(id)
            );

            CREATE INDEX IF NOT EXISTS idx_outcomes_commitment ON outcomes(commitment_id);

            CREATE TABLE IF NOT EXISTS anticipations (
                id                    TEXT PRIMARY KEY,
                generated_at          TEXT NOT NULL,
                trigger_json          TEXT NOT NULL,
                kind                  TEXT NOT NULL,
                predicted_commitment  TEXT,
                grounded_in           TEXT NOT NULL DEFAULT '[]',
                confidence            REAL NOT NULL,
                surfaced_at           TEXT,
                user_response         TEXT,
                eventual_match        TEXT,
                expires_at            TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_anticipations_kind     ON anticipations(kind);
            CREATE INDEX IF NOT EXISTS idx_anticipations_expires  ON anticipations(expires_at);

            CREATE TABLE IF NOT EXISTS context_snapshots (
                id           TEXT PRIMARY KEY,
                captured_at  TEXT NOT NULL,
                blob_json    TEXT NOT NULL
            );

            -- Mined / candidate commitments awaiting user confirmation.
            -- See `docs/INTENT_SYSTEM.md` §3.1. Candidates are NOT
            -- Commitments yet — only on `accept_candidate` do they
            -- promote into the `commitments` table with an Open state.
            -- Statuses: 'pending' (default) | 'accepted' | 'dismissed'.
            CREATE TABLE IF NOT EXISTS commitment_candidates (
                id              TEXT PRIMARY KEY,
                created_at      TEXT NOT NULL,
                kind            TEXT NOT NULL,
                statement       TEXT NOT NULL,
                source_text     TEXT NOT NULL,
                matched_phrase  TEXT NOT NULL,
                span_start      INTEGER NOT NULL,
                span_end        INTEGER NOT NULL,
                confidence      REAL NOT NULL,
                tags            TEXT NOT NULL DEFAULT '[]',
                status          TEXT NOT NULL DEFAULT 'pending',
                resolved_at     TEXT,
                promoted_to     TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_candidates_status      ON commitment_candidates(status);
            CREATE INDEX IF NOT EXISTS idx_candidates_created_at  ON commitment_candidates(created_at);

            -- Pattern-detector cell silences. Per `INTENT_SYSTEM.md`
            -- §5.1.3 + §6.2, when the user dismisses a surfaced
            -- pattern we suppress the same *cell shape* for some
            -- window (default 90 days). Cell identity is the stable
            -- 16-hex prefix of a hash over the canonical-JSON of the
            -- `tm_reflect::CellKey` struct — the brief and the CLI
            -- both compute it the same way so the user can pass the
            -- short id from a brief render straight to silence/unsilence.
            -- `silenced_until` is RFC3339 UTC; an expired row is a
            -- no-op (filtered at read time, GC'd lazily on insert).
            CREATE TABLE IF NOT EXISTS pattern_silences (
                cell_hash      TEXT PRIMARY KEY,
                silenced_at    TEXT NOT NULL,
                silenced_until TEXT NOT NULL,
                cell_label     TEXT NOT NULL DEFAULT '',
                reason         TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_silences_until ON pattern_silences(silenced_until);

            -- Insight silences. Per `INTENT_SYSTEM.md` §6.2 (the user
            -- can silence any prediction surface), and per the
            -- TM-INTENT-006 insights panel: a warning/tailwind insight
            -- on an open commitment can be silenced so it stops
            -- surfacing on subsequent briefs. Unit of silence is the
            -- *commitment_id* — once the user says "I get it, stop
            -- highlighting this row", we suppress every insight tone
            -- for that commitment until the silence window expires.
            -- Silencing is per-row, not per-cell, because the
            -- TM-INTENT-006 detector compares an individual outlook
            -- against the global completed-rate baseline; cells aren't
            -- the right grain.
            CREATE TABLE IF NOT EXISTS insight_silences (
                commitment_id  TEXT PRIMARY KEY,
                silenced_at    TEXT NOT NULL,
                silenced_until TEXT NOT NULL,
                reason         TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_insight_silences_until ON insight_silences(silenced_until);
            "#,
        )?;
        Ok(())
    }

    // ----- commitments ----------------------------------------------------

    /// Insert a new commitment. Errors if the id already exists.
    pub fn insert_commitment(&self, c: &Commitment) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO commitments (
                id, kind, statement, made_at, horizon, context_snapshot_id,
                options_considered, chosen, confidence, expected_outcome,
                stakes, state, outcome_id, derived_from, tags, source
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                c.id.to_string(),
                kind_to_str(c.kind),
                c.statement,
                c.made_at.to_rfc3339(),
                c.horizon.map(|t| t.to_rfc3339()),
                c.context_snapshot_id.map(|u| u.to_string()),
                serde_json::to_string(&c.options_considered)?,
                c.chosen,
                c.confidence as f64,
                c.expected_outcome,
                stakes_to_str(c.stakes),
                state_to_str(c.state),
                c.outcome_id.map(|u| u.to_string()),
                serde_json::to_string(&c.derived_from)?,
                serde_json::to_string(&c.tags)?,
                source_to_str(c.source),
            ],
        )?;
        Ok(())
    }

    /// Fetch a commitment by id.
    pub fn get_commitment(&self, id: Uuid) -> Result<Option<Commitment>> {
        self.conn
            .query_row(
                "SELECT id, kind, statement, made_at, horizon, context_snapshot_id,
                        options_considered, chosen, confidence, expected_outcome,
                        stakes, state, outcome_id, derived_from, tags, source
                 FROM commitments WHERE id = ?",
                params![id.to_string()],
                row_to_commitment,
            )
            .optional()
            .map_err(StoreError::from)
            .and_then(|opt| opt.transpose())
    }

    /// Persist `state` + `outcome_id` for an existing commitment.
    /// Caller is expected to have run [`crate::state::transition`] first.
    pub fn update_state(
        &self,
        id: Uuid,
        new_state: State,
        outcome_id: Option<Uuid>,
    ) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE commitments SET state = ?, outcome_id = ? WHERE id = ?",
            params![
                state_to_str(new_state),
                outcome_id.map(|u| u.to_string()),
                id.to_string()
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                kind: "commitment",
                id,
            });
        }
        Ok(())
    }

    /// Open commitments, newest first. Used by the daily brief.
    pub fn list_open(&self, limit: usize) -> Result<Vec<Commitment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, statement, made_at, horizon, context_snapshot_id,
                    options_considered, chosen, confidence, expected_outcome,
                    stakes, state, outcome_id, derived_from, tags, source
             FROM commitments WHERE state IN ('open', 'acted')
             ORDER BY made_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_commitment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Open commitments whose horizon is `<= now` (overdue + due-today),
    /// soonest horizon first. Brief uses this to surface "needs attention".
    /// Commitments with no horizon are excluded — they have no due date.
    pub fn list_overdue_open(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<Commitment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, statement, made_at, horizon, context_snapshot_id,
                    options_considered, chosen, confidence, expected_outcome,
                    stakes, state, outcome_id, derived_from, tags, source
             FROM commitments
             WHERE state IN ('open', 'acted')
               AND horizon IS NOT NULL
               AND horizon <= ?
             ORDER BY horizon ASC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339(), limit as i64], row_to_commitment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Recently completed/abandoned/superseded commitments — i.e. anything
    /// in a terminal state. Returns newest-first by `made_at`. Brief shows
    /// "resolved yesterday".
    pub fn list_recent_resolved(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<Commitment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, statement, made_at, horizon, context_snapshot_id,
                    options_considered, chosen, confidence, expected_outcome,
                    stakes, state, outcome_id, derived_from, tags, source
             FROM commitments
             WHERE state IN ('completed', 'abandoned', 'superseded')
               AND made_at >= ?
             ORDER BY made_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![since.to_rfc3339(), limit as i64], row_to_commitment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Completed commitments with their attached outcome's polarity.
    /// Used by the pattern detector (`tm-reflect::PatternDetector`) per
    /// `docs/INTENT_SYSTEM.md` §5.1 — pattern bucketing operates over
    /// `(commitment, polarity)` pairs and only the `Completed` state
    /// has a guaranteed `outcome_id`.
    ///
    /// `since` is the window-start (typically `now - 12 months`).
    /// Newest first. Inner-joined on `outcomes.id = commitments.outcome_id`
    /// so abandoned/superseded commitments are excluded — they have no
    /// `Polarity::Better/Worse/...` to score against.
    pub fn list_completed_with_polarity(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<(Commitment, Polarity)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.kind, c.statement, c.made_at, c.horizon, c.context_snapshot_id,
                    c.options_considered, c.chosen, c.confidence, c.expected_outcome,
                    c.stakes, c.state, c.outcome_id, c.derived_from, c.tags, c.source,
                    o.polarity
             FROM commitments c
             JOIN outcomes o ON o.id = c.outcome_id
             WHERE c.state = 'completed'
               AND c.made_at >= ?
             ORDER BY c.made_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![since.to_rfc3339(), limit as i64], |row| {
            // commitment columns 0..16, polarity at 16
            let commitment_result = row_to_commitment(row)?;
            let polarity_str: String = row.get(16)?;
            Ok((commitment_result, polarity_str))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (commitment_result, polarity_str) = r?;
            let commitment = commitment_result?;
            let polarity = parse_polarity(&polarity_str)?;
            out.push((commitment, polarity));
        }
        Ok(out)
    }

    /// Same shape as [`list_completed_with_polarity`] but also returns
    /// the outcome's `observed_at` so callers (the calibration view in
    /// particular) can split rows into in-sample vs out-of-sample
    /// against `model.trained_at`.
    ///
    /// `since` is the window-start applied to `c.made_at` (matches the
    /// existing API). Newest first.
    pub fn list_completed_with_outcome_meta(
        &self,
        since: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> Result<Vec<(Commitment, Polarity, chrono::DateTime<chrono::Utc>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.kind, c.statement, c.made_at, c.horizon, c.context_snapshot_id,
                    c.options_considered, c.chosen, c.confidence, c.expected_outcome,
                    c.stakes, c.state, c.outcome_id, c.derived_from, c.tags, c.source,
                    o.polarity, o.observed_at
             FROM commitments c
             JOIN outcomes o ON o.id = c.outcome_id
             WHERE c.state = 'completed'
               AND c.made_at >= ?
             ORDER BY c.made_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![since.to_rfc3339(), limit as i64], |row| {
            let commitment_result = row_to_commitment(row)?;
            let polarity_str: String = row.get(16)?;
            let observed_str: String = row.get(17)?;
            Ok((commitment_result, polarity_str, observed_str))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (commitment_result, polarity_str, observed_str) = r?;
            let commitment = commitment_result?;
            let polarity = parse_polarity(&polarity_str)?;
            let observed_at = chrono::DateTime::parse_from_rfc3339(&observed_str)
                .map_err(|_| StoreError::Invalid {
                    field: "outcomes.observed_at",
                    value: observed_str.clone(),
                })?
                .with_timezone(&chrono::Utc);
            out.push((commitment, polarity, observed_at));
        }
        Ok(out)
    }

    // ----- pattern silencing ---------------------------------------------

    /// Insert a silence row for the given `cell_hash`. If a silence
    /// already exists, extends it (`silenced_until = max(old,
    /// new_until)`). Idempotent.
    ///
    /// `cell_hash` is opaque to the store — callers (`tm-reflect`)
    /// compute it from a stable hash of `CellKey`. `cell_label` is a
    /// human-readable description carried for the brief / `patterns
    /// list` surfaces; the store does not interpret it.
    pub fn upsert_pattern_silence(
        &self,
        cell_hash: &str,
        cell_label: &str,
        silenced_at: chrono::DateTime<chrono::Utc>,
        silenced_until: chrono::DateTime<chrono::Utc>,
        reason: Option<&str>,
    ) -> Result<()> {
        // Use ON CONFLICT to extend the silence window rather than
        // shrink it — never let a re-silence accidentally shorten
        // the existing block.
        self.conn.execute(
            r#"
            INSERT INTO pattern_silences (cell_hash, silenced_at, silenced_until, cell_label, reason)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(cell_hash) DO UPDATE SET
                silenced_until = MAX(silenced_until, excluded.silenced_until),
                cell_label = excluded.cell_label,
                reason = COALESCE(excluded.reason, pattern_silences.reason)
            "#,
            params![
                cell_hash,
                silenced_at.to_rfc3339(),
                silenced_until.to_rfc3339(),
                cell_label,
                reason,
            ],
        )?;
        Ok(())
    }

    /// Remove a silence (re-enable surfacing for that cell). Returns
    /// `true` if a row was deleted, `false` if no silence existed.
    pub fn remove_pattern_silence(&self, cell_hash: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM pattern_silences WHERE cell_hash = ?", params![cell_hash])?;
        Ok(n > 0)
    }

    /// All non-expired silences as of `now`. Used by the brief to
    /// filter the detector's output.
    pub fn list_active_pattern_silences(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<PatternSilence>> {
        let mut stmt = self.conn.prepare(
            "SELECT cell_hash, silenced_at, silenced_until, cell_label, reason
             FROM pattern_silences
             WHERE silenced_until > ?
             ORDER BY silenced_at DESC",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
            // Pull strings out of rusqlite's Result domain, parse
            // the RFC3339 timestamps after via parse_dt so we keep
            // a single error path.
            let cell_hash: String = row.get(0)?;
            let silenced_at_s: String = row.get(1)?;
            let silenced_until_s: String = row.get(2)?;
            let cell_label: String = row.get(3)?;
            let reason: Option<String> = row.get(4)?;
            Ok((cell_hash, silenced_at_s, silenced_until_s, cell_label, reason))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (cell_hash, silenced_at_s, silenced_until_s, cell_label, reason) = r?;
            out.push(PatternSilence {
                cell_hash,
                silenced_at: parse_dt(&silenced_at_s, "silenced_at")?,
                silenced_until: parse_dt(&silenced_until_s, "silenced_until")?,
                cell_label,
                reason,
            });
        }
        Ok(out)
    }

    // ----- insight silencing ---------------------------------------------

    /// Insert / extend an insight silence for the given commitment.
    /// Same idempotent semantics as [`Self::upsert_pattern_silence`]:
    /// re-silencing an already-silenced commitment can only *extend*
    /// the window, never shorten it.
    ///
    /// `commitment_id` is the *open* commitment whose insight surface
    /// the user wants suppressed; the brief filters its
    /// [`tm_reflect::detect_insights`] output against the active set.
    pub fn upsert_insight_silence(
        &self,
        commitment_id: Uuid,
        silenced_at: chrono::DateTime<chrono::Utc>,
        silenced_until: chrono::DateTime<chrono::Utc>,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO insight_silences (commitment_id, silenced_at, silenced_until, reason)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(commitment_id) DO UPDATE SET
                silenced_until = MAX(silenced_until, excluded.silenced_until),
                reason = COALESCE(excluded.reason, insight_silences.reason)
            "#,
            params![
                commitment_id.to_string(),
                silenced_at.to_rfc3339(),
                silenced_until.to_rfc3339(),
                reason,
            ],
        )?;
        Ok(())
    }

    /// Remove an insight silence (re-enable surfacing for that
    /// commitment). Returns `true` if a row was deleted, `false` if
    /// no silence existed.
    pub fn remove_insight_silence(&self, commitment_id: Uuid) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM insight_silences WHERE commitment_id = ?",
            params![commitment_id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// All non-expired insight silences as of `now`. Brief uses this
    /// to filter its insight panel; CLI / MCP `silenced` commands use
    /// it for the inventory view.
    pub fn list_active_insight_silences(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<InsightSilence>> {
        let mut stmt = self.conn.prepare(
            "SELECT commitment_id, silenced_at, silenced_until, reason
             FROM insight_silences
             WHERE silenced_until > ?
             ORDER BY silenced_at DESC",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
            let commitment_id: String = row.get(0)?;
            let silenced_at_s: String = row.get(1)?;
            let silenced_until_s: String = row.get(2)?;
            let reason: Option<String> = row.get(3)?;
            Ok((commitment_id, silenced_at_s, silenced_until_s, reason))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (commitment_id_s, silenced_at_s, silenced_until_s, reason) = r?;
            let commitment_id = Uuid::parse_str(&commitment_id_s).map_err(|_| {
                StoreError::Invalid {
                    field: "insight_silences.commitment_id",
                    value: commitment_id_s.clone(),
                }
            })?;
            out.push(InsightSilence {
                commitment_id,
                silenced_at: parse_dt(&silenced_at_s, "silenced_at")?,
                silenced_until: parse_dt(&silenced_until_s, "silenced_until")?,
                reason,
            });
        }
        Ok(out)
    }

    /// Count of pending candidates — fast head-count for the brief
    /// without paying to deserialize the full list.
    pub fn count_pending_candidates(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM commitment_candidates WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    // ----- outcomes -------------------------------------------------------

    pub fn insert_outcome(&self, o: &Outcome) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO outcomes (
                id, commitment_id, observed_at, description, polarity,
                surprise, evidence_traces, user_note, source
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                o.id.to_string(),
                o.commitment_id.to_string(),
                o.observed_at.to_rfc3339(),
                o.description,
                polarity_to_str(o.polarity),
                o.surprise as f64,
                serde_json::to_string(&o.evidence_traces)?,
                o.user_note,
                outcome_source_to_str(o.source),
            ],
        )?;
        Ok(())
    }

    pub fn get_outcome(&self, id: Uuid) -> Result<Option<Outcome>> {
        self.conn
            .query_row(
                "SELECT id, commitment_id, observed_at, description, polarity,
                        surprise, evidence_traces, user_note, source
                 FROM outcomes WHERE id = ?",
                params![id.to_string()],
                row_to_outcome,
            )
            .optional()
            .map_err(StoreError::from)
            .and_then(|opt| opt.transpose())
    }

    // ----- snapshots ------------------------------------------------------

    pub fn insert_snapshot(&self, snap: &ContextSnapshot) -> Result<()> {
        self.conn.execute(
            "INSERT INTO context_snapshots (id, captured_at, blob_json) VALUES (?, ?, ?)",
            params![
                snap.id.to_string(),
                snap.captured_at.to_rfc3339(),
                serde_json::to_string(snap)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_snapshot(&self, id: Uuid) -> Result<Option<ContextSnapshot>> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT blob_json FROM context_snapshots WHERE id = ?",
                params![id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    // ----- anticipations --------------------------------------------------

    pub fn insert_anticipation(&self, a: &Anticipation) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO anticipations (
                id, generated_at, trigger_json, kind, predicted_commitment,
                grounded_in, confidence, surfaced_at, user_response,
                eventual_match, expires_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                a.id.to_string(),
                a.generated_at.to_rfc3339(),
                serde_json::to_string(&a.trigger)?,
                anticipation_kind_to_str(a.kind),
                a.predicted_commitment
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                serde_json::to_string(&a.grounded_in)?,
                a.confidence as f64,
                a.surfaced_at.map(|t| t.to_rfc3339()),
                a.user_response.map(user_response_to_str),
                a.eventual_match.map(|u| u.to_string()),
                a.expires_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    // ----- candidates -----------------------------------------------------

    /// Insert one mined candidate. The id is generated server-side so
    /// callers don't need to think about it. Status starts `pending`.
    pub fn insert_candidate(&self, cand: &CandidateRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO commitment_candidates (
                id, created_at, kind, statement, source_text,
                matched_phrase, span_start, span_end, confidence,
                tags, status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending')
            "#,
            params![
                cand.id.to_string(),
                cand.created_at.to_rfc3339(),
                kind_to_str(cand.kind),
                cand.statement,
                cand.source_text,
                cand.matched_phrase,
                cand.span.0 as i64,
                cand.span.1 as i64,
                cand.confidence as f64,
                serde_json::to_string(&cand.tags)?,
            ],
        )?;
        Ok(())
    }

    /// Bulk-insert all candidates from one capture event. Returns how
    /// many rows were written. Wrapped in a transaction so a single
    /// SQL error rolls back the batch.
    pub fn insert_candidates(&mut self, cands: &[CandidateRecord]) -> Result<usize> {
        if cands.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        for cand in cands {
            tx.execute(
                r#"
                INSERT INTO commitment_candidates (
                    id, created_at, kind, statement, source_text,
                    matched_phrase, span_start, span_end, confidence,
                    tags, status
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending')
                "#,
                params![
                    cand.id.to_string(),
                    cand.created_at.to_rfc3339(),
                    kind_to_str(cand.kind),
                    cand.statement,
                    cand.source_text,
                    cand.matched_phrase,
                    cand.span.0 as i64,
                    cand.span.1 as i64,
                    cand.confidence as f64,
                    serde_json::to_string(&cand.tags)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(cands.len())
    }

    /// List the most recent `limit` candidates with `status = 'pending'`.
    /// Newest first — the brief consumes them in this order.
    pub fn list_pending_candidates(&self, limit: usize) -> Result<Vec<CandidateRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, kind, statement, source_text,
                    matched_phrase, span_start, span_end, confidence, tags
             FROM commitment_candidates
             WHERE status = 'pending'
             ORDER BY datetime(created_at) DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_candidate)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Promote a candidate into a real Commitment. Generates a fresh
    /// Commitment id (the candidate id is *not* reused — they live in
    /// different tables), inserts it as `Open`, and stamps the
    /// candidate's `status='accepted'` with the new commitment id.
    /// Returns the newly created commitment id.
    pub fn accept_candidate(&mut self, candidate_id: Uuid) -> Result<Uuid> {
        let cand = self
            .get_candidate(candidate_id)?
            .ok_or_else(|| StoreError::NotFound {
                kind: "candidate",
                id: candidate_id,
            })?;
        let mut commitment = Commitment::new(cand.kind, cand.statement.clone(), Source::ImplicitMined);
        commitment.tags = cand.tags.clone();
        commitment.confidence = cand.confidence;
        let new_id = commitment.id;

        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO commitments (
                id, kind, statement, made_at, horizon, context_snapshot_id,
                options_considered, chosen, confidence, expected_outcome,
                stakes, state, outcome_id, derived_from, tags, source
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                commitment.id.to_string(),
                kind_to_str(commitment.kind),
                commitment.statement,
                commitment.made_at.to_rfc3339(),
                commitment.horizon.map(|t| t.to_rfc3339()),
                commitment.context_snapshot_id.map(|u| u.to_string()),
                serde_json::to_string(&commitment.options_considered)?,
                commitment.chosen,
                commitment.confidence as f64,
                commitment.expected_outcome,
                stakes_to_str(commitment.stakes),
                state_to_str(commitment.state),
                commitment.outcome_id.map(|u| u.to_string()),
                serde_json::to_string(&commitment.derived_from)?,
                serde_json::to_string(&commitment.tags)?,
                source_to_str(commitment.source),
            ],
        )?;
        tx.execute(
            "UPDATE commitment_candidates SET status='accepted', resolved_at=?, promoted_to=?
             WHERE id=?",
            params![
                chrono::Utc::now().to_rfc3339(),
                new_id.to_string(),
                candidate_id.to_string(),
            ],
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    /// Mark a candidate dismissed. Idempotent — dismissing an already-
    /// dismissed or accepted row is a no-op (returns `false`).
    pub fn dismiss_candidate(&self, candidate_id: Uuid) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE commitment_candidates SET status='dismissed', resolved_at=?
             WHERE id=? AND status='pending'",
            params![chrono::Utc::now().to_rfc3339(), candidate_id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Lookup helper. Returns `None` if the row is missing.
    pub fn get_candidate(&self, id: Uuid) -> Result<Option<CandidateRecord>> {
        self.conn
            .query_row(
                "SELECT id, created_at, kind, statement, source_text,
                        matched_phrase, span_start, span_end, confidence, tags
                 FROM commitment_candidates WHERE id = ?",
                params![id.to_string()],
                row_to_candidate,
            )
            .optional()
            .map_err(StoreError::from)
            .and_then(|opt| opt.transpose())
    }
}

/// In-store representation of a mined candidate. Distinct from
/// [`crate::miner::MinedCandidate`] because the persisted row carries
/// an `id`, a `created_at`, and an originating `source_text` (so the
/// One row of the `pattern_silences` table — a user choice to
/// suppress a particular pattern-detector cell shape for some
/// window. See `INTENT_SYSTEM.md` §5.1.3 (the user can silence a
/// cell-shape; the detector excludes silenced cells for 90 days by
/// default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternSilence {
    /// Stable identity of the cell — the brief / CLI both compute
    /// it from `tm_reflect::CellKey` so the user can pass the short
    /// id from a brief render straight to silence/unsilence.
    pub cell_hash: String,
    pub silenced_at: chrono::DateTime<chrono::Utc>,
    pub silenced_until: chrono::DateTime<chrono::Utc>,
    /// Snapshot of the human-readable label at silence-time —
    /// stored so `tracemind patterns silenced` can render history
    /// without re-deriving from a possibly-evolved CellKey schema.
    pub cell_label: String,
    /// Optional reason the user gave (`--reason "noisy"`).
    pub reason: Option<String>,
}

/// One row of the `insight_silences` table — a user choice to
/// suppress all insight surfaces (warning + tailwind) for a single
/// commitment for some window. See `INTENT_SYSTEM.md` §6.2 +
/// `tm_reflect::insights` for the surface this silences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsightSilence {
    /// The open commitment whose insight we're suppressing.
    pub commitment_id: Uuid,
    pub silenced_at: chrono::DateTime<chrono::Utc>,
    pub silenced_until: chrono::DateTime<chrono::Utc>,
    /// Optional reason the user gave.
    pub reason: Option<String>,
}

/// brief can show "you said …" with one click to re-open the trace).
#[derive(Debug, Clone)]
pub struct CandidateRecord {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub kind: CommitmentKind,
    pub statement: String,
    /// Full text the candidate was mined from (clipboard / shell line /
    /// MCP turn). Bounded by capture-side truncation; we don't enforce
    /// a limit here.
    pub source_text: String,
    pub matched_phrase: String,
    pub span: (usize, usize),
    pub confidence: f32,
    pub tags: Vec<String>,
}

impl CandidateRecord {
    /// Convenience constructor: bind a [`crate::miner::MinedCandidate`]
    /// to the originating capture text and stamp `created_at = now`.
    pub fn from_mined(cand: &crate::miner::MinedCandidate, source_text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            kind: cand.draft.kind,
            statement: cand.draft.statement.clone(),
            source_text: source_text.into(),
            matched_phrase: cand.matched_phrase.to_string(),
            span: cand.span,
            confidence: cand.confidence,
            tags: cand.draft.tags.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Row → domain converters
// ---------------------------------------------------------------------------

fn row_to_commitment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Commitment>> {
    let id_s: String = row.get(0)?;
    let kind_s: String = row.get(1)?;
    let statement: String = row.get(2)?;
    let made_at_s: String = row.get(3)?;
    let horizon_s: Option<String> = row.get(4)?;
    let snap_s: Option<String> = row.get(5)?;
    let options_json: String = row.get(6)?;
    let chosen: String = row.get(7)?;
    let confidence: f64 = row.get(8)?;
    let expected_outcome: Option<String> = row.get(9)?;
    let stakes_s: String = row.get(10)?;
    let state_s: String = row.get(11)?;
    let outcome_id_s: Option<String> = row.get(12)?;
    let derived_from_json: String = row.get(13)?;
    let tags_json: String = row.get(14)?;
    let source_s: String = row.get(15)?;

    Ok((|| -> Result<Commitment> {
        Ok(Commitment {
            id: parse_uuid(&id_s, "id")?,
            kind: parse_kind(&kind_s)?,
            statement,
            made_at: parse_dt(&made_at_s, "made_at")?,
            horizon: horizon_s.as_deref().map(|s| parse_dt(s, "horizon")).transpose()?,
            context_snapshot_id: snap_s
                .as_deref()
                .map(|s| parse_uuid(s, "context_snapshot_id"))
                .transpose()?,
            options_considered: serde_json::from_str(&options_json)?,
            chosen,
            confidence: confidence as f32,
            expected_outcome,
            stakes: parse_stakes(&stakes_s)?,
            state: parse_state(&state_s)?,
            outcome_id: outcome_id_s
                .as_deref()
                .map(|s| parse_uuid(s, "outcome_id"))
                .transpose()?,
            derived_from: serde_json::from_str(&derived_from_json)?,
            tags: serde_json::from_str(&tags_json)?,
            source: parse_source(&source_s)?,
        })
    })())
}

fn row_to_outcome(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Outcome>> {
    let id_s: String = row.get(0)?;
    let cid_s: String = row.get(1)?;
    let observed_s: String = row.get(2)?;
    let description: String = row.get(3)?;
    let polarity_s: String = row.get(4)?;
    let surprise: f64 = row.get(5)?;
    let evidence_json: String = row.get(6)?;
    let user_note: Option<String> = row.get(7)?;
    let source_s: String = row.get(8)?;

    Ok((|| -> Result<Outcome> {
        Ok(Outcome {
            id: parse_uuid(&id_s, "id")?,
            commitment_id: parse_uuid(&cid_s, "commitment_id")?,
            observed_at: parse_dt(&observed_s, "observed_at")?,
            description,
            polarity: parse_polarity(&polarity_s)?,
            surprise: surprise as f32,
            evidence_traces: serde_json::from_str(&evidence_json)?,
            user_note,
            source: parse_outcome_source(&source_s)?,
        })
    })())
}

fn row_to_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<CandidateRecord>> {
    let id_s: String = row.get(0)?;
    let created_s: String = row.get(1)?;
    let kind_s: String = row.get(2)?;
    let statement: String = row.get(3)?;
    let source_text: String = row.get(4)?;
    let matched_phrase: String = row.get(5)?;
    let span_start: i64 = row.get(6)?;
    let span_end: i64 = row.get(7)?;
    let confidence: f64 = row.get(8)?;
    let tags_json: String = row.get(9)?;

    Ok((|| -> Result<CandidateRecord> {
        Ok(CandidateRecord {
            id: parse_uuid(&id_s, "id")?,
            created_at: parse_dt(&created_s, "created_at")?,
            kind: parse_kind(&kind_s)?,
            statement,
            source_text,
            matched_phrase,
            span: (span_start as usize, span_end as usize),
            confidence: confidence as f32,
            tags: serde_json::from_str(&tags_json)?,
        })
    })())
}

// ---------------------------------------------------------------------------
// String <-> enum lookups
//
// We round-trip through fixed strings rather than serde-on-enums so that
// SQL queries can filter on stable values (e.g. `WHERE state = 'open'`).
// The string forms match the JSON wire format in INTENT_SYSTEM.md §8.
// ---------------------------------------------------------------------------

fn kind_to_str(k: CommitmentKind) -> &'static str {
    match k {
        CommitmentKind::Intent => "intent",
        CommitmentKind::Decision => "decision",
        CommitmentKind::Hypothesis => "hypothesis",
    }
}
fn parse_kind(s: &str) -> Result<CommitmentKind> {
    Ok(match s {
        "intent" => CommitmentKind::Intent,
        "decision" => CommitmentKind::Decision,
        "hypothesis" => CommitmentKind::Hypothesis,
        other => {
            return Err(StoreError::Invalid {
                field: "kind",
                value: other.to_string(),
            })
        }
    })
}

fn state_to_str(s: State) -> &'static str {
    match s {
        State::Open => "open",
        State::Acted => "acted",
        State::Completed => "completed",
        State::Abandoned => "abandoned",
        State::Superseded => "superseded",
    }
}
fn parse_state(s: &str) -> Result<State> {
    Ok(match s {
        "open" => State::Open,
        "acted" => State::Acted,
        "completed" => State::Completed,
        "abandoned" => State::Abandoned,
        "superseded" => State::Superseded,
        other => {
            return Err(StoreError::Invalid {
                field: "state",
                value: other.to_string(),
            })
        }
    })
}

fn stakes_to_str(s: Stakes) -> &'static str {
    match s {
        Stakes::Low => "low",
        Stakes::Medium => "medium",
        Stakes::High => "high",
        Stakes::Reversible => "reversible",
    }
}
fn parse_stakes(s: &str) -> Result<Stakes> {
    Ok(match s {
        "low" => Stakes::Low,
        "medium" => Stakes::Medium,
        "high" => Stakes::High,
        "reversible" => Stakes::Reversible,
        other => {
            return Err(StoreError::Invalid {
                field: "stakes",
                value: other.to_string(),
            })
        }
    })
}

fn source_to_str(s: Source) -> &'static str {
    match s {
        Source::Manual => "manual",
        Source::VoiceCapture => "voice_capture",
        Source::ImplicitMined => "implicit_mined",
        Source::McpStructured => "mcp_structured",
        Source::Cli => "cli",
    }
}
fn parse_source(s: &str) -> Result<Source> {
    Ok(match s {
        "manual" => Source::Manual,
        "voice_capture" => Source::VoiceCapture,
        "implicit_mined" => Source::ImplicitMined,
        "mcp_structured" => Source::McpStructured,
        "cli" => Source::Cli,
        other => {
            return Err(StoreError::Invalid {
                field: "source",
                value: other.to_string(),
            })
        }
    })
}

fn polarity_to_str(p: Polarity) -> &'static str {
    match p {
        Polarity::Better => "better",
        Polarity::AsExpected => "as_expected",
        Polarity::Worse => "worse",
        Polarity::Mixed => "mixed",
        Polarity::NoOutcome => "no_outcome",
    }
}
fn parse_polarity(s: &str) -> Result<Polarity> {
    Ok(match s {
        "better" => Polarity::Better,
        "as_expected" => Polarity::AsExpected,
        "worse" => Polarity::Worse,
        "mixed" => Polarity::Mixed,
        "no_outcome" => Polarity::NoOutcome,
        other => {
            return Err(StoreError::Invalid {
                field: "polarity",
                value: other.to_string(),
            })
        }
    })
}

fn outcome_source_to_str(s: OutcomeSource) -> &'static str {
    match s {
        OutcomeSource::UserPrompted => "user_prompted",
        OutcomeSource::ImplicitMatched => "implicit_matched",
        OutcomeSource::McpStructured => "mcp_structured",
        OutcomeSource::Cli => "cli",
    }
}
fn parse_outcome_source(s: &str) -> Result<OutcomeSource> {
    Ok(match s {
        "user_prompted" => OutcomeSource::UserPrompted,
        "implicit_matched" => OutcomeSource::ImplicitMatched,
        "mcp_structured" => OutcomeSource::McpStructured,
        "cli" => OutcomeSource::Cli,
        other => {
            return Err(StoreError::Invalid {
                field: "outcome_source",
                value: other.to_string(),
            })
        }
    })
}

fn anticipation_kind_to_str(k: AnticipationKind) -> &'static str {
    match k {
        AnticipationKind::PrefetchQuery => "prefetch_query",
        AnticipationKind::PatternMatch => "pattern_match",
        AnticipationKind::Recommendation => "recommendation",
    }
}

fn user_response_to_str(r: UserResponse) -> &'static str {
    match r {
        UserResponse::Accepted => "accepted",
        UserResponse::Dismissed => "dismissed",
        UserResponse::Starred => "starred",
        UserResponse::Silenced => "silenced",
        UserResponse::Ignored => "ignored",
    }
}

fn parse_uuid(s: &str, field: &'static str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| StoreError::Invalid {
        field,
        value: s.to_string(),
    })
}

fn parse_dt(s: &str, field: &'static str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|_| StoreError::Invalid {
            field,
            value: s.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::transition;
    use crate::types::{CommitmentKind, OutcomeSource, Polarity, Source};

    fn fresh_store() -> IntentStore {
        IntentStore::open_in_memory().expect("open in-memory")
    }

    #[test]
    fn schema_creates_idempotently() {
        // Opening twice on the same path must not error (idempotent
        // CREATE TABLE IF NOT EXISTS).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intents.db");
        let _ = IntentStore::open(&path).unwrap();
        let _ = IntentStore::open(&path).unwrap();
    }

    #[test]
    fn round_trips_commitment() {
        let store = fresh_store();
        let mut c = Commitment::new(
            CommitmentKind::Decision,
            "Use Postgres for v2",
            Source::McpStructured,
        );
        c.options_considered = vec!["Postgres".into(), "Mongo".into()];
        c.tags = vec!["infra".into(), "v2".into()];
        c.stakes = Stakes::High;
        c.expected_outcome = Some("scales to 100k/day".into());
        c.confidence = 0.9;

        store.insert_commitment(&c).unwrap();

        let got = store.get_commitment(c.id).unwrap().expect("found");
        assert_eq!(got.id, c.id);
        assert_eq!(got.statement, c.statement);
        assert_eq!(got.kind, CommitmentKind::Decision);
        assert_eq!(got.options_considered, c.options_considered);
        assert_eq!(got.tags, c.tags);
        assert_eq!(got.stakes, Stakes::High);
        assert_eq!(got.expected_outcome.as_deref(), Some("scales to 100k/day"));
        assert!((got.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn round_trips_outcome_and_updates_state() {
        let store = fresh_store();
        let mut c =
            Commitment::new(CommitmentKind::Intent, "ship Friday", Source::Manual);
        store.insert_commitment(&c).unwrap();

        // Walk Open → Acted, persist.
        transition(&mut c, State::Acted, None).unwrap();
        store.update_state(c.id, c.state, c.outcome_id).unwrap();

        // Build outcome, walk Acted → Completed, persist both.
        let o = Outcome::new(c.id, Polarity::Better, "shipped Wed", OutcomeSource::UserPrompted);
        store.insert_outcome(&o).unwrap();
        transition(&mut c, State::Completed, Some(&o)).unwrap();
        store.update_state(c.id, c.state, c.outcome_id).unwrap();

        let got = store.get_commitment(c.id).unwrap().unwrap();
        assert_eq!(got.state, State::Completed);
        assert_eq!(got.outcome_id, Some(o.id));

        let got_o = store.get_outcome(o.id).unwrap().unwrap();
        assert_eq!(got_o.commitment_id, c.id);
        assert_eq!(got_o.polarity, Polarity::Better);
    }

    #[test]
    fn list_open_returns_open_and_acted_only() {
        let store = fresh_store();

        let a = Commitment::new(CommitmentKind::Intent, "open one", Source::Manual);
        let mut b = Commitment::new(CommitmentKind::Intent, "acted one", Source::Manual);
        let mut done = Commitment::new(CommitmentKind::Intent, "done one", Source::Manual);
        let mut bailed = Commitment::new(CommitmentKind::Intent, "bailed one", Source::Manual);

        store.insert_commitment(&a).unwrap();
        store.insert_commitment(&b).unwrap();
        store.insert_commitment(&done).unwrap();
        store.insert_commitment(&bailed).unwrap();

        transition(&mut b, State::Acted, None).unwrap();
        store.update_state(b.id, b.state, None).unwrap();

        let o = Outcome::new(done.id, Polarity::AsExpected, "fine", OutcomeSource::UserPrompted);
        store.insert_outcome(&o).unwrap();
        transition(&mut done, State::Completed, Some(&o)).unwrap();
        store.update_state(done.id, done.state, done.outcome_id).unwrap();

        transition(&mut bailed, State::Abandoned, None).unwrap();
        store.update_state(bailed.id, bailed.state, None).unwrap();

        let opens = store.list_open(10).unwrap();
        assert_eq!(opens.len(), 2);
        let ids: std::collections::HashSet<Uuid> = opens.iter().map(|c| c.id).collect();
        assert!(ids.contains(&a.id));
        assert!(ids.contains(&b.id));
        assert!(!ids.contains(&done.id));
        assert!(!ids.contains(&bailed.id));
    }

    #[test]
    fn update_state_on_unknown_id_errors() {
        let store = fresh_store();
        let err = store
            .update_state(Uuid::new_v4(), State::Acted, None)
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { kind: "commitment", .. }));
    }

    // ----- candidate tests -----

    fn make_candidate(text: &str) -> Vec<crate::store::CandidateRecord> {
        crate::miner::mine(text)
            .iter()
            .map(|m| crate::store::CandidateRecord::from_mined(m, text.to_string()))
            .collect()
    }

    #[test]
    fn candidates_round_trip_pending_list() {
        let mut store = fresh_store();
        let cands = make_candidate("I'll fix the auth bug today. I decided to use bcrypt.");
        assert_eq!(cands.len(), 2, "miner produced two candidates");
        let n = store.insert_candidates(&cands).unwrap();
        assert_eq!(n, 2);

        let pending = store.list_pending_candidates(10).unwrap();
        assert_eq!(pending.len(), 2);
        // Newest first — both have similar created_at (microsecond diff)
        // so we just confirm both are present.
        let ids: Vec<_> = pending.iter().map(|p| p.id).collect();
        for c in &cands {
            assert!(ids.contains(&c.id));
        }
    }

    #[test]
    fn accept_candidate_promotes_to_open_commitment() {
        let mut store = fresh_store();
        let cands = make_candidate("I'm going to ship Sprint C end of next week.");
        store.insert_candidates(&cands).unwrap();

        let cand_id = cands[0].id;
        let new_id = store.accept_candidate(cand_id).unwrap();

        // The new commitment exists and is Open + ImplicitMined.
        let promoted = store.get_commitment(new_id).unwrap().expect("promoted");
        assert_eq!(promoted.state, State::Open);
        assert!(matches!(promoted.source, Source::ImplicitMined));
        assert!(promoted.statement.contains("ship Sprint C"));

        // The candidate row is now 'accepted' (no longer pending).
        let still_pending = store.list_pending_candidates(10).unwrap();
        assert!(still_pending.iter().all(|c| c.id != cand_id));
    }

    #[test]
    fn dismiss_candidate_is_idempotent() {
        let mut store = fresh_store();
        let cands = make_candidate("Probably finishing this tomorrow.");
        store.insert_candidates(&cands).unwrap();
        let cid = cands[0].id;

        let first = store.dismiss_candidate(cid).unwrap();
        assert!(first, "first dismiss returns true");
        let second = store.dismiss_candidate(cid).unwrap();
        assert!(!second, "second dismiss is a no-op");

        // No longer in pending list.
        let pending = store.list_pending_candidates(10).unwrap();
        assert!(pending.iter().all(|c| c.id != cid));
    }

    #[test]
    fn accept_unknown_candidate_errors() {
        let mut store = fresh_store();
        let err = store.accept_candidate(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, StoreError::NotFound { kind: "candidate", .. }));
    }

    // ----- brief-query tests -----

    #[test]
    fn list_overdue_open_returns_only_open_or_acted_with_past_horizon() {
        use chrono::{Duration, Utc};
        let store = fresh_store();
        let now = Utc::now();

        // Past horizon, open → expected.
        let mut past = Commitment::new(CommitmentKind::Intent, "past due", Source::Manual);
        past.horizon = Some(now - Duration::hours(2));
        store.insert_commitment(&past).unwrap();

        // Future horizon, open → excluded.
        let mut future = Commitment::new(CommitmentKind::Intent, "later", Source::Manual);
        future.horizon = Some(now + Duration::days(3));
        store.insert_commitment(&future).unwrap();

        // Past horizon but completed → excluded.
        let mut done = Commitment::new(CommitmentKind::Intent, "already done", Source::Manual);
        done.horizon = Some(now - Duration::hours(5));
        store.insert_commitment(&done).unwrap();
        let o = Outcome::new(done.id, Polarity::AsExpected, "ok", OutcomeSource::UserPrompted);
        store.insert_outcome(&o).unwrap();
        transition(&mut done, State::Completed, Some(&o)).unwrap();
        store.update_state(done.id, done.state, done.outcome_id).unwrap();

        // No horizon → excluded (no due date).
        let no_horizon = Commitment::new(CommitmentKind::Intent, "loose", Source::Manual);
        store.insert_commitment(&no_horizon).unwrap();

        let overdue = store.list_overdue_open(now, 10).unwrap();
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].id, past.id);
    }

    #[test]
    fn list_overdue_open_orders_soonest_horizon_first() {
        use chrono::{Duration, Utc};
        let store = fresh_store();
        let now = Utc::now();

        let mut a = Commitment::new(CommitmentKind::Intent, "earliest", Source::Manual);
        a.horizon = Some(now - Duration::hours(10));
        store.insert_commitment(&a).unwrap();

        let mut b = Commitment::new(CommitmentKind::Intent, "middle", Source::Manual);
        b.horizon = Some(now - Duration::hours(5));
        store.insert_commitment(&b).unwrap();

        let mut c = Commitment::new(CommitmentKind::Intent, "latest", Source::Manual);
        c.horizon = Some(now - Duration::hours(1));
        store.insert_commitment(&c).unwrap();

        let overdue = store.list_overdue_open(now, 10).unwrap();
        assert_eq!(overdue.len(), 3);
        assert_eq!(overdue[0].id, a.id, "earliest horizon first");
        assert_eq!(overdue[1].id, b.id);
        assert_eq!(overdue[2].id, c.id);
    }

    #[test]
    fn list_recent_resolved_returns_terminal_states_since_cutoff() {
        use chrono::{Duration, Utc};
        let store = fresh_store();
        let now = Utc::now();

        // Old completed (before cutoff) → excluded.
        let mut old = Commitment::new(CommitmentKind::Intent, "old", Source::Manual);
        old.made_at = now - Duration::days(30);
        store.insert_commitment(&old).unwrap();
        let oo = Outcome::new(old.id, Polarity::AsExpected, "fine", OutcomeSource::UserPrompted);
        store.insert_outcome(&oo).unwrap();
        transition(&mut old, State::Completed, Some(&oo)).unwrap();
        store.update_state(old.id, old.state, old.outcome_id).unwrap();

        // Recent abandoned → included.
        let mut bailed = Commitment::new(CommitmentKind::Intent, "bailed", Source::Manual);
        bailed.made_at = now - Duration::hours(2);
        store.insert_commitment(&bailed).unwrap();
        transition(&mut bailed, State::Abandoned, None).unwrap();
        store.update_state(bailed.id, bailed.state, None).unwrap();

        // Recent open → excluded (not terminal).
        let still_open = Commitment::new(CommitmentKind::Intent, "still open", Source::Manual);
        store.insert_commitment(&still_open).unwrap();

        let cutoff = now - Duration::days(1);
        let recent = store.list_recent_resolved(cutoff, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, bailed.id);
    }

    #[test]
    fn count_pending_candidates_excludes_accepted_and_dismissed() {
        let mut store = fresh_store();
        let cands = make_candidate("I'll fix the auth bug today. I decided to use bcrypt.");
        assert_eq!(cands.len(), 2);
        store.insert_candidates(&cands).unwrap();
        assert_eq!(store.count_pending_candidates().unwrap(), 2);

        store.dismiss_candidate(cands[0].id).unwrap();
        assert_eq!(store.count_pending_candidates().unwrap(), 1);

        store.accept_candidate(cands[1].id).unwrap();
        assert_eq!(store.count_pending_candidates().unwrap(), 0);
    }

    // ----- insight silences -----------------------------------------------

    #[test]
    fn insight_silence_round_trips_via_active_list() {
        let store = fresh_store();
        let cid = Uuid::new_v4();
        let now = chrono::Utc::now();
        let until = now + chrono::Duration::days(30);

        store
            .upsert_insight_silence(cid, now, until, Some("noisy"))
            .unwrap();

        let active = store.list_active_insight_silences(now).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].commitment_id, cid);
        assert_eq!(active[0].reason.as_deref(), Some("noisy"));
    }

    #[test]
    fn insight_silence_extension_only_grows_window_never_shrinks() {
        let store = fresh_store();
        let cid = Uuid::new_v4();
        let now = chrono::Utc::now();
        let far = now + chrono::Duration::days(90);
        let near = now + chrono::Duration::days(7);

        store.upsert_insight_silence(cid, now, far, None).unwrap();
        // Re-silence with a shorter window — must NOT shrink the
        // existing block. Mirrors the pattern_silences semantics.
        store.upsert_insight_silence(cid, now, near, None).unwrap();

        let active = store.list_active_insight_silences(now).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(
            active[0].silenced_until.timestamp(),
            far.timestamp(),
            "re-silence with a shorter window must not shrink the existing block"
        );
    }

    #[test]
    fn expired_insight_silences_are_excluded_from_active_list() {
        let store = fresh_store();
        let cid = Uuid::new_v4();
        let now = chrono::Utc::now();
        let past_until = now - chrono::Duration::days(1);

        store
            .upsert_insight_silence(cid, now - chrono::Duration::days(60), past_until, None)
            .unwrap();
        let active = store.list_active_insight_silences(now).unwrap();
        assert!(active.is_empty(), "expired silence must be filtered out");
    }

    #[test]
    fn remove_insight_silence_returns_true_then_false() {
        let store = fresh_store();
        let cid = Uuid::new_v4();
        let now = chrono::Utc::now();
        store
            .upsert_insight_silence(cid, now, now + chrono::Duration::days(30), None)
            .unwrap();

        assert!(store.remove_insight_silence(cid).unwrap());
        // Idempotent: second remove is a no-op.
        assert!(!store.remove_insight_silence(cid).unwrap());
        let active = store.list_active_insight_silences(now).unwrap();
        assert!(active.is_empty());
    }
}
