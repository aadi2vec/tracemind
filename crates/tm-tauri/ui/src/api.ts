import { invoke } from "@tauri-apps/api/core";

// Types matching Rust IPC responses

export interface EntityInfo {
  id: string;
  name: string;
  entity_type: string;
  confidence: number;
  /** 2026-05-11 UX (#1) — context this entity was ingested under, by name. */
  context_name: string | null;
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

export interface RecommendationInfo {
  entity_id: string;
  entity_name: string;
  entity_type: string;
  score: number;
  reason: string;
  /** 2026-05-11 UX (#3) — structured why ("PR 0.72 · 86% match"). */
  reason_detail: string;
  /** 2026-05-11 UX (#1) — query text that seeded this rec. */
  origin_query: string | null;
  /** 2026-05-11 UX (#1) — context this rec was computed under. */
  origin_context: string | null;
}

export interface AttributionInfo {
  entity_id: string;
  entity_name: string;
  source: string;
  weight: number;
}

export interface QueryResponse {
  /** Sprint C-0.7 / F-1 — stable id of this query for feedback wiring. */
  query_id: string;
  arm: number;
  arm_name: string;
  latency_ms: number;
  entities: EntityInfo[];
  triples: TripleInfo[];
  recommendations: RecommendationInfo[];
  explanation: string;
  attributions: AttributionInfo[];
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

export interface GraphNode {
  id: string;
  name: string;
  entity_type: string;
  confidence: number;
  community: number | null;
}

export interface GraphEdge {
  source: string;
  target: string;
  predicate: string;
  confidence: number;
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
  community_count: number;
}

export interface SurprisingEntity {
  entity_id: string;
  entity_name: string;
  entity_type: string;
  novelty: number;
  recency: number;
  score: number;
}

export interface TrendPoint {
  date: string;
  entity_count: number;
}

export interface DemoResult {
  texts_ingested: number;
  total_entities: number;
  total_triples: number;
}

export async function getGraph(): Promise<GraphData> {
  return invoke("cmd_graph");
}

export async function demoIngest(): Promise<DemoResult> {
  return invoke("cmd_demo_ingest");
}

export async function entityClick(entityId: string): Promise<void> {
  return invoke("cmd_entity_click", { entityId });
}

export async function getRecommendations(limit?: number): Promise<RecommendationInfo[]> {
  return invoke("cmd_recommendations", { limit: limit ?? 5 });
}

// 2026-05-11 UX (#2) — cold-start seed for the Reasoning Engine.
export interface ReasonSeed {
  entity_id: string;
  entity_name: string;
  entity_type: string;
  why: string;
}

export async function reasonSeed(): Promise<ReasonSeed | null> {
  return invoke("cmd_reason_seed");
}

// 2026-05-11 UX (#4) — recent queries for the QueryView sticky panel.
export interface RecentQueryInfo {
  trace_id: string;
  query_text: string;
  arm_name: string | null;
  entities_count: number;
  created_at: string;
}

export async function getRecentQueries(limit?: number): Promise<RecentQueryInfo[]> {
  return invoke("cmd_query_recent", { limit: limit ?? 5 });
}

export async function sendFeedback(score: number): Promise<void> {
  return invoke("cmd_feedback", { score });
}

export async function toggleCapture(enabled: boolean): Promise<void> {
  return invoke("cmd_toggle_capture", { enabled });
}

export async function getCaptureStatus(): Promise<boolean> {
  return invoke("cmd_capture_status");
}

export async function deleteEntity(entityId: string): Promise<void> {
  return invoke("cmd_delete_entity", { entityId });
}

export async function getSurprising(limit?: number): Promise<SurprisingEntity[]> {
  return invoke("cmd_surprising", { limit: limit ?? 5 });
}

export async function getEntityTrends(): Promise<TrendPoint[]> {
  return invoke("cmd_entity_trends");
}

// Reasoning types

export interface ReasoningStepInfo {
  entity_id: string;
  entity_name: string;
  entity_type: string;
  predicate: string;
  direction: string;
  confidence: number;
}

export interface ReasoningChainInfo {
  steps: ReasoningStepInfo[];
  score: number;
  source_id: string;
  destination_id: string;
}

export interface AnalogyInfo {
  source_name: string;
  target_id: string;
  target_name: string;
  similarity: number;
  shared_patterns: string[];
  explanation: string;
}

export async function reasonChain(sourceName: string, targetName: string): Promise<ReasoningChainInfo[]> {
  return invoke("cmd_reason_chain", { sourceName, targetName });
}

export async function reasonExplore(entityName: string, maxResults?: number): Promise<ReasoningChainInfo[]> {
  return invoke("cmd_reason_explore", { entityName, maxResults: maxResults ?? 10 });
}

export async function findAnalogies(entityName: string, maxResults?: number): Promise<AnalogyInfo[]> {
  return invoke("cmd_find_analogies", { entityName, maxResults: maxResults ?? 5 });
}

export interface ConsolidationResult {
  entities_strengthened: number;
  entities_decayed: number;
  entities_pruned: number;
  entities_merged: number;
  triples_pruned: number;
}

export async function consolidateMemory(): Promise<ConsolidationResult> {
  return invoke("cmd_consolidate");
}

export interface CaptureEvent {
  source: string;
  text: string;
  entities_count: number;
  triples_count: number;
}

// Daily brief — D-4

export interface BriefCounts {
  overdue: number;
  open: number;
  resolved: number;
  candidates: number;
  patterns: number;
  insights: number;
  proposals: number;
  outcome_prompts: number;
  contradictions: number;
}

export interface BriefRow {
  id: string;
  title: string;
  horizon: string | null;
  state: string;
  polarity: string | null;
  overdue_class: string | null;
}

export interface ContradictionRow {
  id: string;
  triple_a: string;
  triple_b: string;
  detected_at: string;
  cosine_similarity: number;
}

export interface BriefView {
  generated_at: string;
  counts: BriefCounts;
  overdue: BriefRow[];
  open: BriefRow[];
  resolved: BriefRow[];
  contradictions: ContradictionRow[];
}

export async function getBrief(): Promise<BriefView> {
  return invoke("cmd_brief");
}

// Contradiction drawer (E series)

export interface TripleDetailView {
  triple_id: string;
  subject_id: string;
  subject_name: string;
  subject_type: string;
  predicate: string;
  object_id: string;
  object_name: string;
  object_type: string;
  confidence: number;
  source_id: string | null;
  ingested_at: string;
  status: string | null;
}

export type ResolveChoice = "keep_a" | "keep_b" | "keep_both";

export interface ResolveContradictionResult {
  retracted: string[];
  kept: string[];
}

export async function getTripleDetail(
  tripleId: string,
): Promise<TripleDetailView | null> {
  return invoke("cmd_triple_detail", { tripleId });
}

export async function resolveContradiction(
  tripleA: string,
  tripleB: string,
  choice: ResolveChoice,
): Promise<ResolveContradictionResult> {
  return invoke("cmd_resolve_contradiction", {
    tripleA,
    tripleB,
    choice,
  });
}

// Outcome-prompt drawer (E series)

export type OutcomePolarity =
  | "better"
  | "as_expected"
  | "worse"
  | "mixed"
  | "no_outcome";

export interface RecordOutcomeResult {
  outcome_id: string;
  commitment_id: string;
  commitment_state: string;
  polarity: string;
}

export async function recordOutcome(
  commitmentId: string,
  polarity: OutcomePolarity,
  description?: string,
  userNote?: string,
): Promise<RecordOutcomeResult> {
  return invoke("cmd_record_outcome", {
    commitmentId,
    polarity,
    description,
    userNote,
  });
}

// Sprint D / F-1 — feedback channels + context CRUD

export interface FeedbackAck {
  row_id: number;
  kind: string;
}

export async function markHelpful(
  queryId: string,
  resultId: string,
  weight?: number,
  kind?: string,
): Promise<FeedbackAck> {
  return invoke("cmd_helpful", { queryId, resultId, weight, kind });
}

export async function markNotRelated(
  queryId: string,
  resultId: string,
  weight?: number,
  kind?: string,
): Promise<FeedbackAck> {
  return invoke("cmd_not_related", { queryId, resultId, weight, kind });
}

export interface ContextInfo {
  id: string;
  name: string;
  tags: string;
  is_active: boolean;
}

export async function listContexts(): Promise<ContextInfo[]> {
  return invoke("cmd_context_list");
}

export async function currentContext(): Promise<ContextInfo | null> {
  return invoke("cmd_context_current");
}

export async function useContext(name: string): Promise<ContextInfo> {
  return invoke("cmd_context_use", { name });
}

export async function createContext(
  name: string,
  tags?: string,
): Promise<ContextInfo> {
  return invoke("cmd_context_create", { name, tags });
}

export async function clearContext(): Promise<void> {
  return invoke("cmd_context_clear");
}
