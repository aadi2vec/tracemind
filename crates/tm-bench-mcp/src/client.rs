//! A minimal JSON-RPC 2.0 client that drives the real `tm-mcp` binary.
//!
//! This is what makes the harness honest: rather than testing an in-process
//! stub, it spawns the actual server the way a host does — as a subprocess
//! speaking newline-delimited JSON-RPC over stdin/stdout — and measures what
//! a host would actually experience, including process latency.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

/// A running `tm-mcp` subprocess with an open JSON-RPC channel.
pub struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpServer {
    /// Spawn `binary` with `TM_DATA_DIR=data_dir`. When `advanced` is set,
    /// the server advertises its full tool surface.
    pub fn spawn(binary: &str, data_dir: &str, advanced: bool) -> Result<Self> {
        let mut cmd = Command::new(binary);
        cmd.env("TM_DATA_DIR", data_dir)
            .env("TM_HASH_EMBED", "1") // deterministic + no model download in CI
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if advanced {
            cmd.env("TM_MCP_ADVANCED", "1");
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning MCP server at {binary}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        Ok(Self { child, stdin, stdout, next_id: 1 })
    }

    /// Send a request and read the matching response, returning it with the
    /// round-trip latency.
    pub fn request(&mut self, method: &str, params: Value) -> Result<(Value, Duration)> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

        let start = Instant::now();
        writeln!(self.stdin, "{}", serde_json::to_string(&req)?)
            .context("writing request")?;
        self.stdin.flush().context("flushing request")?;

        // Read lines until we get a JSON object carrying our id. The server
        // may interleave notifications; skip anything that isn't our reply.
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).context("reading response")?;
            if n == 0 {
                return Err(anyhow!("server closed the connection before replying to {method}"));
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_str::<Value>(line) else {
                continue; // not JSON (stray log line) — ignore
            };
            if val.get("id").and_then(|v| v.as_i64()) == Some(id) {
                let elapsed = start.elapsed();
                if let Some(err) = val.get("error") {
                    return Err(anyhow!("JSON-RPC error on {method}: {err}"));
                }
                return Ok((val.get("result").cloned().unwrap_or(Value::Null), elapsed));
            }
        }
    }

    pub fn initialize(&mut self) -> Result<Value> {
        let (r, _) = self.request(
            "initialize",
            json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "tm-bench-mcp", "version": "0.1.0" } }),
        )?;
        Ok(r)
    }

    /// The advertised tool list as `(name, description)` pairs.
    pub fn list_tools(&mut self) -> Result<Vec<(String, String)>> {
        let (r, _) = self.request("tools/list", json!({}))?;
        let tools = r
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| anyhow!("tools/list returned no `tools` array"))?;
        Ok(tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
                Some((name, desc))
            })
            .collect())
    }

    /// Call a tool, returning the result and latency.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<(Value, Duration)> {
        self.request("tools/call", json!({ "name": name, "arguments": arguments }))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// p-th percentile (0..100) of a set of durations, in milliseconds.
pub fn percentile_ms(durations: &[Duration], p: f64) -> f64 {
    if durations.is_empty() {
        return 0.0;
    }
    let mut ms: Vec<f64> = durations.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (p / 100.0 * (ms.len() - 1) as f64).round() as usize;
    ms[rank.min(ms.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_of_empty_is_zero() {
        assert_eq!(percentile_ms(&[], 95.0), 0.0);
    }

    #[test]
    fn percentile_picks_the_right_rank() {
        let ds: Vec<Duration> = (1..=100).map(|n| Duration::from_millis(n)).collect();
        // p50 of 1..100 ms is ~50 ms, p95 ~95 ms.
        assert!((percentile_ms(&ds, 50.0) - 50.0).abs() <= 1.0);
        assert!((percentile_ms(&ds, 95.0) - 95.0).abs() <= 1.0);
    }
}
