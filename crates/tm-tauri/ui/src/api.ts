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
  /// 2026-05-12 — stringified community_id → "Top1 · Top2 · Top3"
  /// label built from highest-degree entity names in that community.
  /// Used by the Graph legend so each community is human-readable
  /// instead of "Community 0/1/2/…".
  community_labels: Record<string, string>;
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

// Next Actions — verb-first action feed (2026-05-11)
//
// Replaces Dashboard "Suggested for You" entity recs with verb cards
// synthesised from commitments + contradictions in cmd_brief.

export type NextActionKind = "Resolve" | "FollowUp" | "Review" | "Confirm" | "Connect";
export type NextActionPriority = "overdue" | "normal" | "low";
export type NextActionTargetKind = "commitment" | "contradiction" | "entity" | "candidate";

export interface NextActionInfo {
  id: string;
  kind: NextActionKind;
  verb: string;
  title: string;
  subtitle: string | null;
  target_id: string;
  target_kind: NextActionTargetKind;
  priority: NextActionPriority;
}

export async function getNextActions(): Promise<NextActionInfo[]> {
  return invoke("cmd_next_actions");
}

/// Promote a pending mined commitment candidate to an Open Commitment.
/// Returns the new commitment uuid as a string.
export async function acceptCandidate(id: string): Promise<string> {
  return invoke("cmd_accept_candidate", { id });
}

/// Dismiss a pending mined candidate. Returns true if a row was flipped
/// (false = already accepted / dismissed). Idempotent.
export async function dismissCandidate(id: string): Promise<boolean> {
  return invoke("cmd_dismiss_candidate", { id });
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

// UI-13 — Capture permissions panel

export interface CapturePermissionRow {
  source: string;
  description: string;
  enabled: boolean;
  default_enabled: boolean;
  granted_at: string | null;
  last_event_at: string | null;
  event_count: number;
}

export async function listCapturePermissions(): Promise<CapturePermissionRow[]> {
  return invoke("cmd_capture_permissions_list");
}

export async function setCapturePermission(source: string, enabled: boolean): Promise<void> {
  return invoke("cmd_capture_permissions_set", { source, enabled });
}

export interface ForgetSourceResult {
  source: string;
  entities_removed: number;
  traces_redacted: number;
}

export async function forgetCaptureSource(source: string): Promise<ForgetSourceResult> {
  return invoke("cmd_capture_forget_source", { source });
}

// DP-3 — usage instrumentation

export interface UsageStats {
  first_seen: string | null;
  last_query_at: string | null;
  last_helpful_at: string | null;
  last_negative_at: string | null;
  total_queries: number;
  total_helpful: number;
  total_negative: number;
  active_days: string[];
}

export async function getUsageStats(): Promise<UsageStats> {
  return invoke("cmd_usage_stats");
}

export async function getUsageSharePayload(): Promise<string> {
  return invoke("cmd_usage_share_payload");
}

// UI-14 — Context-switch suggestion

export interface ContextSuggestion {
  suggested_context: string;
  confidence: number;
  reason: string;
}

export async function suggestContext(queryText: string): Promise<ContextSuggestion | null> {
  return invoke("cmd_context_suggest", { queryText });
}

// ---------------------------------------------------------------------------
// P5 legibility primitives — Entity Drawer / Transclusion / Thread View /
// Memory Garden / Outliers / Community Overlay
// ---------------------------------------------------------------------------

// LM-3 — Entity Drawer composite view
export interface EntityDrawerHeader {
  entity_id: string;
  name: string;
  entity_type: string;
  ontological_domain: string;
  confidence: number;
  created_at: string;
  updated_at: string;
}

export interface EntityDrawerBacklink {
  triple_id: string;
  source_id: string;
  source_name: string;
  predicate: string;
  confidence: number;
}

export interface EntityDrawerRelation {
  triple_id: string;
  target_id: string;
  target_name: string;
  predicate: string;
  confidence: number;
}

export interface EntityDrawerView {
  header: EntityDrawerHeader;
  backlinks: EntityDrawerBacklink[];
  relations: EntityDrawerRelation[];
  tags: string[];
}

export async function getEntityDrawer(
  entityId: string,
  backlinkLimit?: number,
): Promise<EntityDrawerView> {
  return invoke("cmd_entity_drawer", { entityId, backlinkLimit });
}

// LM-4 — entity-updated event payload (emitted by capture loop)
export interface EntityUpdatedEvent {
  entity_ids: string[];
  source: string;
}

// LM-5a — Transclusion (`![[entity_id]]`) resolver
export interface TransclusionSpan {
  start: number;
  end: number;
  entity_id: string | null;
  entity_name: string | null;
  preview: string | null;
}

export async function resolveTransclusion(text: string): Promise<TransclusionSpan[]> {
  return invoke("cmd_resolve_transclusion", { text });
}

// LM-11e — per-thread saved splice state
export interface ThreadViewState {
  view_name: string | null;
  include_ids: string[];
  exclude_ids: string[];
}

export async function saveThreadView(
  threadId: string,
  state: ThreadViewState,
): Promise<void> {
  return invoke("cmd_thread_view_save", {
    threadId,
    viewName: state.view_name,
    includeIds: state.include_ids,
    excludeIds: state.exclude_ids,
  });
}

export async function loadThreadView(threadId: string): Promise<ThreadViewState> {
  return invoke("cmd_thread_view_load", { threadId });
}

export async function clearThreadView(threadId: string): Promise<boolean> {
  return invoke("cmd_thread_view_clear", { threadId });
}

// LM-11e — Views surface: list every saved splice
export interface ThreadViewRow {
  thread_id: string;
  view_name: string | null;
  include_count: number;
  exclude_count: number;
}

export async function listThreadViews(): Promise<ThreadViewRow[]> {
  return invoke("cmd_thread_views_list");
}

// LM-20/22 — Memory Garden cards (cluster buckets + outlier tray)
export interface GardenCard {
  cluster_id: number | null;
  label: string;
  count: number;
  sample_texts: string[];
}

export async function getMemoryGarden(): Promise<GardenCard[]> {
  return invoke("cmd_memory_garden");
}

// LM-22 — outlier tray rows
export interface OutlierRow {
  signal_id: number;
  raw_text: string;
  source: string;
  created_at: string;
}

export async function listOutliers(limit?: number): Promise<OutlierRow[]> {
  return invoke("cmd_outliers_list", { limit });
}

export type OutlierAction = "add_to" | "new_cluster" | "ignore";

export async function triageOutlier(
  signalId: number,
  action: OutlierAction,
  targetCluster?: number,
): Promise<number> {
  return invoke("cmd_outlier_triage", { signalId, action, targetCluster });
}

// LM-23 — Community overlay rows
export interface CommunityRow {
  community_id: number | null;
  entity_count: number;
  sample_names: string[];
  /// 2026-05-12 — short "Top1 · Top2 · Top3" label (top sample
  /// entities). Empty for the unassigned bucket.
  label: string;
}

export async function getCommunityOverlay(): Promise<CommunityRow[]> {
  return invoke("cmd_community_overlay");
}

// ───────────────────────────────────────────────────────────────────────
// Context Dashboard — fat one-call payload for "everything we know about X"
// ───────────────────────────────────────────────────────────────────────

export interface ContextDecay {
  recency: number;
  novelty: number;
  value: number;
  frequency: number;
  access_count: number;
  last_access: string | null;
}

export interface ContextNeighbor {
  entity_id: string;
  name: string;
  entity_type: string;
  similarity: number;
}

export interface ContextHop {
  entity_id: string;
  name: string;
  entity_type: string;
}

export interface ContextBeliefRow {
  triple_id: string;
  subject: string;
  predicate: string;
  object: string;
  /** "In" | "Out" | "Contradicted" | "Unknown" */
  status: string;
}

export interface ContextContradiction {
  id: string;
  triple_a: string;
  triple_b: string;
  detected_at: string;
  cosine_similarity: number;
  /** "KeepA" | "KeepB" | "KeepBoth" | null */
  resolution: string | null;
}

export interface ContextProvenanceRow {
  name: string;
  entity_type: string;
  confidence: number;
  valid_from: string;
  recorded_at: string;
  superseded_at: string | null;
}

export interface ContextTraceRow {
  trace_id: string;
  event_type: string;
  raw_text: string | null;
  retrieval_arm: number | null;
  created_at: string;
}

export interface ContextSignal {
  signal_id: number;
  raw_text: string;
  source: string;
  similarity: number;
  created_at: string;
}

export interface ContextCommunity {
  community_id: number | null;
  label: string;
  sibling_count: number;
  sibling_names: string[];
}

export interface ContextReasoningStep {
  entity_id: string;
  entity_name: string;
  predicate: string;
  /** "Out" | "In" */
  direction: string;
  confidence: number;
}

export interface ContextReasoningChain {
  target_id: string;
  target_name: string;
  score: number;
  steps: ContextReasoningStep[];
}

export interface ContextAnalogy {
  target_id: string;
  target_name: string;
  similarity: number;
  shared_patterns: string[];
  explanation: string;
}

export interface ContextBanditUse {
  arm: number;
  arm_name: string;
  pulls: number;
}

export interface ContextIntentRow {
  id: string;
  statement: string;
  state: string;
  horizon: string | null;
}

export interface EntityContextDump {
  header: EntityDrawerHeader;
  decay: ContextDecay;
  relations_out: EntityDrawerRelation[];
  relations_in: EntityDrawerBacklink[];
  vector_neighbors: ContextNeighbor[];
  k_hop_neighbors: ContextHop[];
  community: ContextCommunity;
  belief_rows: ContextBeliefRow[];
  contradictions: ContextContradiction[];
  provenance: ContextProvenanceRow[];
  recent_traces: ContextTraceRow[];
  signal_neighbors: ContextSignal[];
  reasoning_chains: ContextReasoningChain[];
  analogies: ContextAnalogy[];
  bandit_arms_used: ContextBanditUse[];
  related_intents: ContextIntentRow[];
}

export async function getEntityContext(
  entityId: string,
): Promise<EntityContextDump> {
  return invoke("cmd_entity_context_dump", { entityId });
}

// ───────────────────────────────────────────────────────────────────────
// Inspector — Brain snapshot + Why-this-answer + Layer browser + MD export
// ───────────────────────────────────────────────────────────────────────

export interface BrainArmRow {
  arm: number;
  name: string;
  ucb_pulls: number;
  ucb_avg_reward: number;
  linucb_pulls: number;
  linucb_weight_mag: number;
}

export interface BrainEventBreakdown {
  event_type: string;
  count: number;
}

export interface BrainSnapshot {
  generated_at: string;
  data_dir: string;
  entity_count: number;
  triple_count: number;
  contradiction_count: number;
  pending_relation_count: number;
  community_count: number;
  vector_dim: number;
  entities_with_vector: number;
  bandit_arms: BrainArmRow[];
  linucb_alpha: number;
  total_traces: number;
  by_event: BrainEventBreakdown[];
  recent_buffer_size: number;
  recent_buffer_capacity: number;
  governance_blocks_today: number;
  captures_today: number;
  open_commitments: number;
  overdue_commitments: number;
  pending_candidates: number;
  pattern_silences_active: number;
}

export async function getBrainSnapshot(): Promise<BrainSnapshot> {
  return invoke("cmd_brain_snapshot");
}

export interface WhyArmRow {
  arm: number;
  name: string;
  top_k: number;
  hops: number;
  include_episodic: boolean;
  include_colbert: boolean;
  exploit_score: number;
  explore_score: number;
  total_score: number;
  pulls: number;
}

export interface WhyEntityRow {
  entity_id: string;
  name: string;
  entity_type: string;
  confidence: number;
}

export interface WhyTraceView {
  trace_id: string;
  event_type: string;
  created_at: string;
  raw_text: string | null;
  plan_action: string | null;
  plan_complexity: string | null;
  plan_confidence: number | null;
  plan_entity_hints: string[];
  selected_arm: number | null;
  selected_arm_name: string | null;
  arm_scores: WhyArmRow[];
  entities: WhyEntityRow[];
  latency_ms: number | null;
  confidence_gate_passed: boolean;
  linucb_alpha: number;
}

export async function getTraceWhy(traceId: string): Promise<WhyTraceView> {
  return invoke("cmd_trace_why", { traceId });
}

export type InspectorLayerName =
  | "vector"
  | "graph"
  | "episodic"
  | "bandit"
  | "governance"
  | "reason";

export type InspectorLayerPayload = Record<string, unknown>;

export async function getInspectorLayer(
  layer: InspectorLayerName,
): Promise<InspectorLayerPayload> {
  return invoke("cmd_inspector_layer", { layer });
}

export interface ExportEntityMarkdownResult {
  output_path: string;
  bytes_written: number;
  entity_id: string;
}

export async function exportEntityMarkdown(
  entityId: string,
  outputPath: string,
): Promise<ExportEntityMarkdownResult> {
  return invoke("cmd_export_entity_markdown", { entityId, outputPath });
}
