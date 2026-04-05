import { invoke } from "@tauri-apps/api/core";

// Types matching Rust IPC responses

export interface EntityInfo {
  id: string;
  name: string;
  entity_type: string;
  confidence: number;
}

export interface TripleInfo {
  subject: string;
  predicate: string;
  object: string;
  confidence: number;
}

export interface IngestResponse {
  trace_id: string;
  entities: EntityInfo[];
  typed_triples: TripleInfo[];
  co_occurrence_count: number;
}

export interface QueryResponse {
  arm: number;
  arm_name: string;
  latency_ms: number;
  entities: EntityInfo[];
  triples: TripleInfo[];
}

export interface TraceInfo {
  id: string;
  event_type: string;
  raw_text: string;
  entities_count: number;
  triples_count: number;
  retrieval_arm: number | null;
  retrieval_arm_name: string | null;
  retrieval_latency_ms: number | null;
  created_at: string;
}

export interface BanditArmInfo {
  arm: number;
  name: string;
  pulls: number;
  avg_reward: number;
}

export interface DashboardStats {
  entity_count: number;
  triple_count: number;
  trace_count: number;
  bandit_arms: BanditArmInfo[];
  recent_traces: TraceInfo[];
}

// API calls

export async function ingestText(text: string): Promise<IngestResponse> {
  return invoke("cmd_ingest", { text });
}

export async function queryMemory(text: string): Promise<QueryResponse> {
  return invoke("cmd_query", { text });
}

export async function getDashboard(): Promise<DashboardStats> {
  return invoke("cmd_dashboard");
}

export async function getTraces(limit?: number): Promise<TraceInfo[]> {
  return invoke("cmd_traces", { limit: limit ?? 50 });
}

export async function decayMemory(factor: number, threshold: number): Promise<string> {
  return invoke("cmd_decay", { factor, threshold });
}
