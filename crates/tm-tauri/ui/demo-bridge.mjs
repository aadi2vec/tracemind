#!/usr/bin/env bun
// Demo bridge — dev-only HTTP shim that lets the Vite browser preview talk to
// the real tracemind backend via the CLI. NOT used in the Tauri production
// build (which uses IPC via `invoke`). Run with:
//   TM_DATA_DIR=/tmp/tm-demo bun demo-bridge.mjs
// Then open the Vite dev server; api.ts detects the absence of Tauri IPC and
// POSTs here instead.

import { spawn } from "node:child_process";
import { readFileSync, existsSync, statSync } from "node:fs";
import { join } from "node:path";

const PORT = 3131;
const REPO = "/Users/aadityasrivathsan/Desktop/agent_memory_system";
const CLI = `${REPO}/target/release/tracemind`;
const DATA_DIR = process.env.TM_DATA_DIR || `${process.env.HOME}/.tracemind`;
const SESSION_START = new Date().toISOString();
let currentMode = { mode: "ambient", session: null };

function runCli(args) {
  return new Promise((resolve, reject) => {
    // Use the real BGE-small ONNX embedder (NOT --hash-embed). The demo must
    // exercise the same semantic retrieval path the product ships with;
    // hash embeddings are deterministic but semantically meaningless
    // (LoCoMo F1 ~48 vs ~70) and would make the demo lie about quality.
    const proc = spawn(CLI, [...args], {
      env: { ...process.env, TM_DATA_DIR: DATA_DIR },
    });
    let stdout = "";
    let stderr = "";
    proc.stdout.on("data", (d) => (stdout += d));
    proc.stderr.on("data", (d) => (stderr += d));
    proc.on("close", (code) => {
      if (code !== 0) return reject(new Error(`cli ${args.join(" ")} exit ${code}: ${stderr}`));
      resolve(stdout);
    });
  });
}

function parseIngest(out) {
  // "  Trace:    <uuid>" and "    [Type] Name" lines
  const traceMatch = out.match(/Trace:\s+([0-9a-f-]+)/);
  const entities = [];
  for (const line of out.split("\n")) {
    const m = line.match(/^\s+\[([^\]]+)\]\s+(.+)$/);
    if (m) {
      entities.push({
        id: crypto.randomUUID(),
        name: m[2].trim(),
        entity_type: m[1].trim(),
        confidence: 0.9,
        context_name: null,
      });
    }
  }
  return {
    trace_id: traceMatch ? traceMatch[1] : crypto.randomUUID(),
    entities,
    typed_triples: [],
    co_occurrence_count: 0,
  };
}

function parseQuery(out) {
  const idMatch = out.match(/Query:\s+([0-9a-f-]+)/);
  const armMatch = out.match(/Answer\s+\(([^,]+),\s+(\d+)ms\)/);
  const answerMatch = out.match(/Answer\s+\([^)]+\):\n(.+?)(?:\n\nCitations|\n\n\s+\[|\Z)/s);
  const citationsMatch = out.match(/Citations:\s+(.+)/);
  const entities = [];
  const seen = new Set();
  for (const line of out.split("\n")) {
    const m = line.match(/^\s+\[([^\]]+)\]\s+(.+)$/);
    if (m) {
      const key = `${m[1]}:${m[2]}`;
      if (seen.has(key)) continue;
      seen.add(key);
      entities.push({
        id: crypto.randomUUID(),
        name: m[2].trim(),
        entity_type: m[1].trim(),
        confidence: 0.85,
        context_name: null,
      });
    }
  }
  const explanation = answerMatch ? answerMatch[1].trim() : out.trim();
  const citationText = citationsMatch ? citationsMatch[1].trim() : "";
  return {
    query_id: idMatch ? idMatch[1] : crypto.randomUUID(),
    arm: 1,
    arm_name: armMatch ? armMatch[1] : "extractive",
    latency_ms: armMatch ? parseInt(armMatch[2], 10) : 0,
    entities,
    triples: [],
    recommendations: [],
    explanation: citationText ? `${explanation}\n\nCitations: ${citationText}` : explanation,
    attributions: [],
  };
}

function readTraces(limit) {
  const path = join(DATA_DIR, "traces.jsonl");
  if (!existsSync(path)) return [];
  const lines = readFileSync(path, "utf8").trim().split("\n").filter(Boolean);
  const out = [];
  for (const line of lines.reverse().slice(0, limit)) {
    try {
      const t = JSON.parse(line);
      const evtRaw = t.event_type || (t.retrieval_arm != null ? "retrieve" : "ingest");
      const evt = evtRaw.charAt(0).toUpperCase() + evtRaw.slice(1).toLowerCase();
      // Ambient sources emit their payload prefixed with `[[source]]`
      // (in the real app this is MultimodalPayload.source from the
      // ModalitySource that captured it — NotesVaultSource, WebPinSource,
      // CalendarSource, EmailInboxSource, …). Parse it into a badge and
      // strip it from the display text.
      let rawText = t.raw_text || t.query_text || "";
      let source = null;
      const srcMatch = rawText.match(/^\s*\[\[([a-z0-9_-]+)\]\]\s*/i);
      if (srcMatch) {
        source = srcMatch[1].toLowerCase();
        rawText = rawText.slice(srcMatch[0].length);
      }
      out.push({
        id: t.id,
        event_type: evt,
        source,
        raw_text: rawText,
        entities_count: t.entities_extracted?.length ?? t.entities_count ?? 0,
        triples_count: t.triples_extracted?.length ?? t.triples_count ?? 0,
        retrieval_arm: t.retrieval_arm ?? null,
        retrieval_arm_name: t.retrieval_arm_name ?? null,
        retrieval_latency_ms: t.retrieval_latency_ms ?? null,
        created_at: t.created_at || t.timestamp || new Date().toISOString(),
      });
    } catch {}
  }
  return out;
}

const handlers = {
  async cmd_ingest({ text }) {
    const out = await runCli(["ingest", text]);
    return parseIngest(out);
  },
  async cmd_query({ text }) {
    const out = await runCli(["query", text]);
    return parseQuery(out);
  },
  async cmd_traces({ limit }) {
    return readTraces(limit ?? 50);
  },
  async cmd_mode_current() {
    return currentMode;
  },
  async cmd_mode_enter_focus({ intent, durationSecs }) {
    currentMode = {
      mode: "focus",
      session: {
        id: crypto.randomUUID(),
        started_at: new Date().toISOString(),
        duration_secs: durationSecs ?? null,
        intent: intent ?? null,
      },
    };
    return currentMode;
  },
  async cmd_mode_enter_private({ durationSecs }) {
    currentMode = {
      mode: "private",
      session: {
        id: crypto.randomUUID(),
        started_at: new Date().toISOString(),
        duration_secs: durationSecs ?? null,
        intent: null,
      },
    };
    return currentMode;
  },
  async cmd_mode_end() {
    currentMode = { mode: "ambient", session: null };
    return currentMode;
  },
  async cmd_context_list() {
    return [{ name: "default", tags: [], is_current: true }];
  },
  async cmd_context_current() {
    return { name: "default", tags: [] };
  },
  async cmd_brain_snapshot() {
    const traces = readTraces(500);
    const ingestCount = traces.filter((t) => t.event_type === "Ingest").length;
    const retrieveCount = traces.filter((t) => t.event_type === "Retrieve").length;
    return {
      layers: [],
      totals: {
        entities: traces.reduce((s, t) => s + (t.entities_count || 0), 0),
        triples: traces.reduce((s, t) => s + (t.triples_count || 0), 0),
        ingest_events: ingestCount,
        retrieve_events: retrieveCount,
      },
    };
  },
};

const server = Bun.serve({
  port: PORT,
  async fetch(req) {
    const cors = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type",
    };
    if (req.method === "OPTIONS") return new Response(null, { headers: cors });
    const url = new URL(req.url);
    if (url.pathname !== "/rpc") return new Response("not found", { status: 404, headers: cors });
    try {
      const { cmd, args } = await req.json();
      const handler = handlers[cmd];
      if (!handler) {
        return new Response(JSON.stringify({ error: `no handler: ${cmd}` }), {
          status: 501,
          headers: { ...cors, "Content-Type": "application/json" },
        });
      }
      const result = await handler(args ?? {});
      return new Response(JSON.stringify(result), {
        headers: { ...cors, "Content-Type": "application/json" },
      });
    } catch (err) {
      return new Response(JSON.stringify({ error: String(err) }), {
        status: 500,
        headers: { ...cors, "Content-Type": "application/json" },
      });
    }
  },
});

console.log(`[demo-bridge] listening on http://localhost:${PORT}  data=${DATA_DIR}`);
