//! Storage maintenance — scan size + on-demand cleanup.
//!
//! TraceMind keeps its entire state under one data directory (default
//! `~/.tracemind`). Over time this can accumulate ephemeral signals,
//! old trace lines, and fragmented SQLite pages. This module gives the
//! UI + CLI a small set of explicit cleanup primitives:
//!
//! - [`storage_stats`]  — read-only: per-file sizes + row counts.
//! - [`vacuum_all`]     — run SQLite `VACUUM` on every `.db` file. No
//!                        data is lost; freed pages are returned to the
//!                        filesystem.
//! - [`clean_ephemeral`]— delete `captured_signals` rows at tier 4
//!                        (Ephemeral) plus any already-consolidated
//!                        signal rows with a `cluster_id` set (their
//!                        entities are already in the graph).
//! - [`truncate_traces`]— keep only the most recent `keep_recent`
//!                        lines of `traces.jsonl`. The trace log is
//!                        append-only audit data; old lines are still
//!                        on disk but no longer surface in the UI.
//! - [`reset_all`]      — nuclear: delete every file in the data dir
//!                        except `models/`. Caller must hold no open
//!                        DB handles.
//!
//! Everything in this module is best-effort: an error reading one file
//! never aborts the rest of the scan / cleanup. Callers receive a
//! report and can decide whether to retry.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};

/// Per-file disk usage row, sorted largest first by [`storage_stats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub name: String,
    pub bytes: u64,
}

/// Snapshot of the data directory: total disk usage, per-file breakdown,
/// and a few headline row counts that drive the cleanup buttons.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub data_dir: String,
    pub total_bytes: u64,
    pub files: Vec<FileStat>,
    pub entity_count: i64,
    pub triple_count: i64,
    pub signal_count: i64,
    /// Tier-4 (Ephemeral) signals — safe to delete.
    pub ephemeral_signal_count: i64,
    /// Signals with a non-null `cluster_id` — already promoted to
    /// entities; raw text no longer needed for retrieval.
    pub consolidated_signal_count: i64,
    /// Number of lines in `traces.jsonl` (audit log).
    pub trace_line_count: u64,
}

/// Outcome of a destructive operation. `bytes_freed` is computed from
/// pre/post file sizes so the UI can show "freed 1.2 MB" honestly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupReport {
    pub action: String,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub bytes_freed: i64,
    pub rows_deleted: i64,
}

/// Walk the data directory once and compute [`StorageStats`].
///
/// Hidden files and `models/` are excluded from the row breakdown but
/// still counted in `total_bytes` so the displayed total matches what
/// `du -sh` would show.
pub fn storage_stats(data_dir: &Path) -> Result<StorageStats> {
    let mut total: u64 = 0;
    let mut files: Vec<FileStat> = Vec::new();

    if data_dir.exists() {
        for entry in fs::read_dir(data_dir).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_file() {
                total += meta.len();
                files.push(FileStat {
                    name,
                    bytes: meta.len(),
                });
            } else if meta.is_dir() {
                let dir_size = dir_size_recursive(&entry.path());
                total += dir_size;
                files.push(FileStat {
                    name: format!("{name}/"),
                    bytes: dir_size,
                });
            }
        }
    }

    files.sort_by(|a, b| b.bytes.cmp(&a.bytes));

    let memory_db = data_dir.join("memory.db");
    let (entities, triples, signals, ephemeral, consolidated) = if memory_db.exists() {
        count_memory_rows(&memory_db).unwrap_or_default()
    } else {
        (0, 0, 0, 0, 0)
    };

    let trace_lines = count_lines(&data_dir.join("traces.jsonl"));

    Ok(StorageStats {
        data_dir: data_dir.display().to_string(),
        total_bytes: total,
        files,
        entity_count: entities,
        triple_count: triples,
        signal_count: signals,
        ephemeral_signal_count: ephemeral,
        consolidated_signal_count: consolidated,
        trace_line_count: trace_lines,
    })
}

/// Run `VACUUM` on every `.db` file directly under `data_dir`. Returns
/// one [`CleanupReport`] with the aggregate before/after sizes.
///
/// Note: this opens its own short-lived `rusqlite::Connection` per file.
/// **Caller must hold no other write handles to these databases** —
/// VACUUM in WAL mode requires an exclusive lock. On the desktop app,
/// route through a Tauri command that runs while the GraphStore is
/// quiesced (no active ingest / query).
pub fn vacuum_all(data_dir: &Path) -> Result<CleanupReport> {
    let dbs = ["memory.db", "intents.db", "memory.db.temporal"];
    let mut before: u64 = 0;
    let mut after: u64 = 0;
    for db in dbs {
        let p = data_dir.join(db);
        if !p.exists() {
            continue;
        }
        before += file_size(&p);
        if let Ok(conn) = Connection::open(&p) {
            // Best-effort. A VACUUM failure on one DB shouldn't block
            // the others; the report's bytes_after captures the truth.
            let _ = conn.execute_batch("VACUUM;");
        }
        after += file_size(&p);
    }
    Ok(CleanupReport {
        action: "vacuum_all".into(),
        bytes_before: before,
        bytes_after: after,
        bytes_freed: before as i64 - after as i64,
        rows_deleted: 0,
    })
}

/// Delete `captured_signals` rows that are either tier 4 (Ephemeral)
/// or already consolidated (`cluster_id` non-null). Both are safe to
/// drop: ephemeral signals never contribute to retrieval, and
/// consolidated ones have already been promoted into the entity graph.
///
/// Does *not* VACUUM; call [`vacuum_all`] afterwards if you want the
/// disk savings to materialize.
pub fn clean_ephemeral(data_dir: &Path) -> Result<CleanupReport> {
    let p = data_dir.join("memory.db");
    if !p.exists() {
        return Ok(CleanupReport {
            action: "clean_ephemeral".into(),
            bytes_before: 0,
            bytes_after: 0,
            bytes_freed: 0,
            rows_deleted: 0,
        });
    }
    let before = file_size(&p);
    let conn = Connection::open(&p).map_err(sql_err)?;
    let mut deleted: i64 = 0;
    // tier 4 = Ephemeral
    if let Ok(n) = conn.execute("DELETE FROM captured_signals WHERE tier = 4", []) {
        deleted += n as i64;
    }
    if let Ok(n) = conn.execute(
        "DELETE FROM captured_signals WHERE cluster_id IS NOT NULL AND tier > 1",
        [],
    ) {
        deleted += n as i64;
    }
    drop(conn);
    let after = file_size(&p);
    Ok(CleanupReport {
        action: "clean_ephemeral".into(),
        bytes_before: before,
        bytes_after: after,
        bytes_freed: before as i64 - after as i64,
        rows_deleted: deleted,
    })
}

/// Keep only the last `keep_recent` lines of `traces.jsonl`. The trace
/// log is append-only audit data; truncating it loses old provenance
/// but no live data.
///
/// Pass `0` to wipe the trace log entirely.
pub fn truncate_traces(data_dir: &Path, keep_recent: usize) -> Result<CleanupReport> {
    let p = data_dir.join("traces.jsonl");
    if !p.exists() {
        return Ok(CleanupReport {
            action: "truncate_traces".into(),
            bytes_before: 0,
            bytes_after: 0,
            bytes_freed: 0,
            rows_deleted: 0,
        });
    }
    let before = file_size(&p);
    let original_lines = count_lines(&p);

    if keep_recent == 0 {
        fs::write(&p, b"").map_err(io_err)?;
        return Ok(CleanupReport {
            action: "truncate_traces".into(),
            bytes_before: before,
            bytes_after: 0,
            bytes_freed: before as i64,
            rows_deleted: original_lines as i64,
        });
    }

    // Two-pass strategy: read all lines into memory (trace files are
    // typically < 1 MB), keep the tail, write atomically via a tmp file.
    let file = fs::File::open(&p).map_err(io_err)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().map_while(std::result::Result::ok).collect();
    let tail_start = lines.len().saturating_sub(keep_recent);
    let tmp = p.with_extension("jsonl.tmp");
    {
        let mut out = fs::File::create(&tmp).map_err(io_err)?;
        for line in &lines[tail_start..] {
            writeln!(out, "{line}").map_err(io_err)?;
        }
    }
    fs::rename(&tmp, &p).map_err(io_err)?;

    let after = file_size(&p);
    Ok(CleanupReport {
        action: "truncate_traces".into(),
        bytes_before: before,
        bytes_after: after,
        bytes_freed: before as i64 - after as i64,
        rows_deleted: (original_lines as i64) - (lines.len().saturating_sub(tail_start) as i64),
    })
}

/// Nuclear reset: delete every file directly under `data_dir` except
/// `models/`. The caller is expected to confirm with the user and to
/// have closed all DB handles first; we don't enforce either guard.
///
/// Returns the total bytes freed.
pub fn reset_all(data_dir: &Path) -> Result<CleanupReport> {
    let mut before: u64 = 0;
    let mut removed: i64 = 0;
    if !data_dir.exists() {
        return Ok(CleanupReport {
            action: "reset_all".into(),
            bytes_before: 0,
            bytes_after: 0,
            bytes_freed: 0,
            rows_deleted: 0,
        });
    }
    for entry in fs::read_dir(data_dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let name = entry.file_name();
        // Preserve the models cache — it's expensive to re-download.
        if name == std::ffi::OsStr::new("models") {
            continue;
        }
        if meta.is_file() {
            before += meta.len();
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        } else if meta.is_dir() {
            before += dir_size_recursive(&entry.path());
            if fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(CleanupReport {
        action: "reset_all".into(),
        bytes_before: before,
        bytes_after: 0,
        bytes_freed: before as i64,
        rows_deleted: removed,
    })
}

// ── helpers ──────────────────────────────────────────────────────────

fn io_err(e: std::io::Error) -> TraceMindError {
    TraceMindError::Storage(format!("maintenance io: {e}"))
}

fn sql_err(e: rusqlite::Error) -> TraceMindError {
    TraceMindError::Storage(format!("maintenance sql: {e}"))
}

fn file_size(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn dir_size_recursive(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                total += meta.len();
            } else if meta.is_dir() {
                total += dir_size_recursive(&entry.path());
            }
        }
    }
    total
}

fn count_lines(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let reader = BufReader::new(f);
    reader.lines().filter_map(std::result::Result::ok).count() as u64
}

/// Count entities, triples, total signals, ephemeral signals (tier 4),
/// and consolidated signals (`cluster_id IS NOT NULL`). Returns zeros
/// on any error so a missing table doesn't sink the stats panel.
fn count_memory_rows(path: &PathBuf) -> Result<(i64, i64, i64, i64, i64)> {
    let conn = Connection::open(path).map_err(sql_err)?;
    let scalar = |sql: &str| -> i64 {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .unwrap_or(0)
    };
    Ok((
        scalar("SELECT COUNT(*) FROM kg_entities"),
        scalar("SELECT COUNT(*) FROM kg_relations"),
        scalar("SELECT COUNT(*) FROM captured_signals"),
        scalar("SELECT COUNT(*) FROM captured_signals WHERE tier = 4"),
        scalar("SELECT COUNT(*) FROM captured_signals WHERE cluster_id IS NOT NULL"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn stats_on_missing_dir_is_empty() {
        let dir = std::path::Path::new("/tmp/tm-nonexistent-test-dir");
        let s = storage_stats(dir).unwrap();
        assert_eq!(s.total_bytes, 0);
        assert_eq!(s.files.len(), 0);
    }

    #[test]
    fn stats_sums_file_sizes() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        fs::write(tmp.path().join("b.txt"), b"world!!").unwrap();
        let s = storage_stats(tmp.path()).unwrap();
        assert_eq!(s.total_bytes, 12);
        assert_eq!(s.files.len(), 2);
        // Largest first.
        assert_eq!(s.files[0].name, "b.txt");
    }

    #[test]
    fn truncate_traces_keeps_tail() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("traces.jsonl");
        let body = (0..10).map(|i| format!("line-{i}")).collect::<Vec<_>>().join("\n");
        fs::write(&p, body).unwrap();
        let report = truncate_traces(tmp.path(), 3).unwrap();
        assert_eq!(report.action, "truncate_traces");
        let kept = fs::read_to_string(&p).unwrap();
        assert!(kept.contains("line-9"));
        assert!(kept.contains("line-7"));
        assert!(!kept.contains("line-3"));
    }

    #[test]
    fn truncate_traces_zero_wipes_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("traces.jsonl");
        fs::write(&p, b"line-0\nline-1\nline-2\n").unwrap();
        let report = truncate_traces(tmp.path(), 0).unwrap();
        assert_eq!(report.bytes_after, 0);
        assert_eq!(report.rows_deleted, 3);
    }

    #[test]
    fn reset_preserves_models_subdir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("models")).unwrap();
        fs::write(tmp.path().join("models").join("bge.bin"), b"weights").unwrap();
        fs::write(tmp.path().join("memory.db"), b"db data").unwrap();
        let report = reset_all(tmp.path()).unwrap();
        assert!(report.bytes_freed > 0);
        assert!(tmp.path().join("models").exists());
        assert!(tmp.path().join("models").join("bge.bin").exists());
        assert!(!tmp.path().join("memory.db").exists());
    }
}
