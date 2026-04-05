import { useState } from "react";
import { queryMemory, type QueryResponse } from "../api";

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

  const handleQuery = async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError("");
    try {
      const res = await queryMemory(query.trim());
      setResult(res);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

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
          {/* Meta bar */}
          <div className="flex items-center gap-4 text-xs text-tm-muted">
            <span>Strategy: <span className="text-tm-accent">{result.arm_name}</span></span>
            <span>Latency: <span className="text-tm-text">{result.latency_ms}ms</span></span>
            <span>Entities: <span className="text-tm-text">{uniqueEntities.length}</span></span>
            <span>Triples: <span className="text-tm-text">{result.triples.length}</span></span>
          </div>

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

          {/* Entity cards */}
          <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
              Entities
            </h3>
            <div className="flex flex-wrap gap-2">
              {uniqueEntities.map((e) => (
                <div
                  key={e.id}
                  className={`px-3 py-1.5 rounded-full text-xs font-medium ${typeColor(e.entity_type)}`}
                >
                  {e.name}
                  <span className="ml-1 opacity-60">{e.entity_type}</span>
                </div>
              ))}
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
        </div>
      )}
    </div>
  );
}
