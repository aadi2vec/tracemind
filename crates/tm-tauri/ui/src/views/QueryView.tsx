import { useState } from "react";
import {
  queryMemory,
  entityClick,
  sendFeedback,
  markHelpful,
  markNotRelated,
  type QueryResponse,
  type AttributionInfo,
} from "../api";

type RowFeedback = "helpful" | "not_related" | "wrong_context";

const TYPE_COLORS: Record<string, string> = {
  Person: "bg-blue-500/20 text-blue-400",
  Organization: "bg-purple-500/20 text-purple-400",
  Technology: "bg-emerald-500/20 text-emerald-400",
  Concept: "bg-slate-500/20 text-slate-400",
  File: "bg-amber-500/20 text-amber-400",
  Url: "bg-cyan-500/20 text-cyan-400",
  Project: "bg-pink-500/20 text-pink-400",
  Decision: "bg-orange-500/20 text-orange-400",
  Event: "bg-rose-500/20 text-rose-400",
};

function typeColor(t: string): string {
  return TYPE_COLORS[t] || "bg-tm-border text-tm-muted";
}

export default function QueryView() {
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<QueryResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [feedbackGiven, setFeedbackGiven] = useState<"up" | "down" | null>(null);
  const [showAttribution, setShowAttribution] = useState(false);
  // Sprint D / UI-5 — per-row feedback. Keyed by entity id so the user
  // can mark multiple rows independently. Optimistic: we render the
  // pill immediately and rely on the backend ack arriving silently.
  const [rowFeedback, setRowFeedback] = useState<Record<string, RowFeedback>>({});

  const handleQuery = async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError("");
    setFeedbackGiven(null);
    setRowFeedback({});
    try {
      const res = await queryMemory(query.trim());
      setResult(res);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  async function fileRowFeedback(
    queryId: string,
    entityId: string,
    kind: RowFeedback,
  ) {
    // Optimistic update.
    setRowFeedback((prev) => ({ ...prev, [entityId]: kind }));
    try {
      if (kind === "helpful") {
        await markHelpful(queryId, entityId);
      } else if (kind === "not_related") {
        await markNotRelated(queryId, entityId, 1.0, "not_related");
      } else {
        await markNotRelated(queryId, entityId, 1.0, "cross_context_bridge");
      }
    } catch (e) {
      // Roll back on failure so the user can retry.
      console.error("row feedback failed:", e);
      setRowFeedback((prev) => {
        const next = { ...prev };
        delete next[entityId];
        return next;
      });
    }
  }

  // Separate typed vs RelatedTo triples
  const typedTriples = result?.triples.filter((t) => t.predicate !== "RelatedTo") ?? [];
  const relatedTriples = result?.triples.filter((t) => t.predicate === "RelatedTo") ?? [];

  // Deduplicate entities by name
  const seen = new Set<string>();
  const uniqueEntities = (result?.entities ?? []).filter((e) => {
    if (seen.has(e.name)) return false;
    seen.add(e.name);
    return true;
  });

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Query Memory</h2>

      {/* Search bar */}
      <div className="flex gap-3">
        <input
          className="flex-1 bg-tm-surface border border-tm-border rounded-lg px-4 py-3 text-sm placeholder-tm-muted"
          placeholder="Ask your memory anything..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleQuery()}
        />
        <button
          onClick={handleQuery}
          disabled={loading}
          className="px-6 py-3 bg-tm-accent text-white rounded-lg text-sm font-medium hover:bg-tm-accent/80 transition-colors disabled:opacity-50"
        >
          {loading ? "Searching..." : "Search"}
        </button>
      </div>

      {error && <div className="text-tm-red text-sm">{error}</div>}

      {result && (
        <div className="space-y-5">
          {/* Meta bar with feedback */}
          <div className="flex items-center gap-4 text-xs text-tm-muted">
            <span>Strategy: <span className="text-tm-accent">{result.arm_name}</span></span>
            <span>Latency: <span className="text-tm-text">{result.latency_ms}ms</span></span>
            <span>Entities: <span className="text-tm-text">{uniqueEntities.length}</span></span>
            <span>Triples: <span className="text-tm-text">{result.triples.length}</span></span>
            <div className="ml-auto flex items-center gap-2">
              <span className="text-tm-muted">Was this helpful?</span>
              <button
                onClick={() => { sendFeedback(1.0); setFeedbackGiven("up"); }}
                disabled={feedbackGiven !== null}
                className={`px-2 py-1 rounded text-sm transition-colors ${
                  feedbackGiven === "up"
                    ? "bg-emerald-500/30 text-emerald-400"
                    : "hover:bg-emerald-500/20 hover:text-emerald-400"
                } ${feedbackGiven !== null && feedbackGiven !== "up" ? "opacity-30" : ""}`}
              >
                +
              </button>
              <button
                onClick={() => { sendFeedback(0.0); setFeedbackGiven("down"); }}
                disabled={feedbackGiven !== null}
                className={`px-2 py-1 rounded text-sm transition-colors ${
                  feedbackGiven === "down"
                    ? "bg-red-500/30 text-red-400"
                    : "hover:bg-red-500/20 hover:text-red-400"
                } ${feedbackGiven !== null && feedbackGiven !== "down" ? "opacity-30" : ""}`}
              >
                -
              </button>
              {feedbackGiven && (
                <span className="text-xs text-tm-muted">Thanks!</span>
              )}
            </div>
          </div>

          {/* Causal attribution — why these results */}
          {result.attributions && result.attributions.length > 0 && (
            <div className="bg-tm-surface border border-tm-border rounded-lg overflow-hidden">
              <button
                onClick={() => setShowAttribution(!showAttribution)}
                className="w-full px-5 py-3 flex items-center justify-between text-left hover:bg-white/5 transition-colors"
              >
                <span className="text-sm font-medium text-tm-muted">
                  Why these results? <span className="text-xs opacity-60">({result.attributions.length} evidence paths)</span>
                </span>
                <span className="text-xs text-tm-muted">{showAttribution ? "▲" : "▼"}</span>
              </button>
              {showAttribution && (
                <div className="px-5 pb-4 space-y-2 border-t border-tm-border pt-3">
                  {result.attributions.map((a: AttributionInfo, i: number) => {
                    const maxWeight = Math.max(...result.attributions.map((x: AttributionInfo) => x.weight));
                    const pct = maxWeight > 0 ? (a.weight / maxWeight) * 100 : 0;
                    return (
                      <div key={i} className="flex items-center gap-3">
                        <div className="w-24 h-1.5 bg-tm-border rounded-full overflow-hidden flex-shrink-0">
                          <div
                            className="h-full bg-tm-accent rounded-full transition-all"
                            style={{ width: `${pct}%` }}
                          />
                        </div>
                        <span className="text-sm font-medium w-32 truncate">{a.entity_name}</span>
                        <span className="text-xs text-tm-muted flex-1">{a.source}</span>
                        <span className="text-xs text-tm-accent font-mono">{(a.weight * 100).toFixed(0)}%</span>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          {/* Typed triples — the important relationships */}
          {typedTriples.length > 0 && (
            <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
                Key Relationships
              </h3>
              <div className="space-y-2">
                {typedTriples.map((t, i) => (
                  <div key={i} className="flex items-center gap-2 text-sm">
                    <span className="font-medium text-tm-text">{t.subject}</span>
                    <span className="px-2 py-0.5 rounded bg-tm-accent/20 text-tm-accent text-xs font-mono">
                      {t.predicate}
                    </span>
                    <span className="font-medium text-tm-text">{t.object}</span>
                    <span className="text-xs text-tm-muted ml-auto">
                      {(t.confidence * 100).toFixed(0)}%
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Entity rows — per-row feedback (Sprint D / UI-5) */}
          <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
              Entities
            </h3>
            <div className="space-y-1.5">
              {uniqueEntities.map((e) => {
                const fb = rowFeedback[e.id];
                const dim = fb !== undefined;
                return (
                  <div
                    key={e.id}
                    className={`flex items-center gap-2 px-2 py-1.5 rounded hover:bg-white/5 transition-colors ${
                      dim ? "opacity-60" : ""
                    }`}
                  >
                    <button
                      onClick={() => entityClick(e.id)}
                      className={`px-2.5 py-1 rounded-full text-xs font-medium cursor-pointer hover:ring-2 hover:ring-tm-accent/50 transition-all ${typeColor(
                        e.entity_type,
                      )}`}
                    >
                      {e.name}
                      <span className="ml-1 opacity-60">{e.entity_type}</span>
                    </button>
                    <span className="text-[10px] text-tm-muted ml-auto">
                      {(e.confidence * 100).toFixed(0)}%
                    </span>
                    {fb ? (
                      <span className="text-[10px] text-tm-muted italic px-2">
                        {fb === "helpful"
                          ? "noted ✓"
                          : fb === "not_related"
                            ? "filed not-related"
                            : "filed wrong-context"}
                      </span>
                    ) : (
                      <div className="flex items-center gap-1">
                        <button
                          onClick={() => fileRowFeedback(result.query_id, e.id, "helpful")}
                          title="Helpful"
                          className="px-1.5 py-0.5 text-xs rounded hover:bg-emerald-500/20 hover:text-emerald-400 transition-colors"
                        >
                          👍
                        </button>
                        <button
                          onClick={() => fileRowFeedback(result.query_id, e.id, "not_related")}
                          title="Not related"
                          className="px-1.5 py-0.5 text-xs rounded hover:bg-red-500/20 hover:text-red-400 transition-colors"
                        >
                          👎
                        </button>
                        <button
                          onClick={() => fileRowFeedback(result.query_id, e.id, "wrong_context")}
                          title="Wrong context (cross-context bridge)"
                          className="px-1.5 py-0.5 text-[10px] rounded text-tm-muted hover:bg-amber-500/20 hover:text-amber-400 transition-colors"
                        >
                          wrong ctx
                        </button>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>

          {/* Co-occurrence graph */}
          {relatedTriples.length > 0 && (
            <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
                Co-occurrence Links ({relatedTriples.length})
              </h3>
              <div className="grid grid-cols-2 gap-1 text-xs text-tm-muted">
                {relatedTriples.slice(0, 20).map((t, i) => (
                  <span key={i}>
                    {t.subject} <span className="text-tm-border">---</span> {t.object}
                  </span>
                ))}
              </div>
            </div>
          )}

          {/* Recommendations */}
          {result.recommendations.length > 0 && (
            <div className="bg-tm-surface border border-tm-accent/20 rounded-lg p-5">
              <h3 className="text-sm font-medium text-tm-accent uppercase tracking-wider mb-3">
                Recommended for You
              </h3>
              <div className="space-y-2">
                {result.recommendations.map((rec) => (
                  <button
                    key={rec.entity_id}
                    onClick={() => {
                      entityClick(rec.entity_id);
                      setQuery(rec.entity_name);
                    }}
                    className="w-full flex items-center gap-3 p-3 rounded bg-tm-bg border border-tm-border hover:border-tm-accent/40 transition-colors text-left"
                  >
                    <div className="flex-1">
                      <span className="text-sm font-medium">{rec.entity_name}</span>
                      <span className="text-xs text-tm-muted ml-2">{rec.entity_type}</span>
                    </div>
                    <span className="text-xs text-tm-muted">{rec.reason}</span>
                    <span className="text-xs text-tm-accent font-mono">
                      {(rec.score * 100).toFixed(0)}%
                    </span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
