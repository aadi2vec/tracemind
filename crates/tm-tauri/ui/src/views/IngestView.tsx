import { useState } from "react";
import { ingestText, type IngestResponse } from "../api";

export default function IngestView() {
  const [text, setText] = useState("");
  const [result, setResult] = useState<IngestResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const handleIngest = async () => {
    if (!text.trim()) return;
    setLoading(true);
    setError("");
    try {
      const res = await ingestText(text.trim());
      setResult(res);
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Ingest Memory</h2>

      <div className="bg-tm-surface border border-tm-border rounded-lg p-5 space-y-4">
        <textarea
          className="w-full bg-tm-bg border border-tm-border rounded-lg px-4 py-3 text-sm placeholder-tm-muted resize-none h-32"
          placeholder="Paste or type a memory to ingest..."
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && e.metaKey) handleIngest();
          }}
        />
        <div className="flex items-center justify-between">
          <span className="text-xs text-tm-muted">Cmd+Enter to submit</span>
          <button
            onClick={handleIngest}
            disabled={loading || !text.trim()}
            className="px-6 py-2.5 bg-tm-accent text-white rounded-lg text-sm font-medium hover:bg-tm-accent/80 transition-colors disabled:opacity-50"
          >
            {loading ? "Ingesting..." : "Ingest"}
          </button>
        </div>
      </div>

      {error && <div className="text-tm-red text-sm">{error}</div>}

      {result && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-5 space-y-4">
          <div className="flex items-center gap-3">
            <div className="w-2 h-2 rounded-full bg-tm-green" />
            <span className="text-sm font-medium">Ingested successfully</span>
            <span className="text-xs text-tm-muted font-mono ml-auto">
              trace: {result.trace_id.slice(0, 8)}
            </span>
          </div>

          {/* Entities */}
          <div>
            <p className="text-xs text-tm-muted uppercase tracking-wider mb-2">
              Entities Extracted ({result.entities.length})
            </p>
            <div className="flex flex-wrap gap-2">
              {result.entities.map((e) => (
                <span
                  key={e.id}
                  className="px-2.5 py-1 rounded bg-tm-bg border border-tm-border text-xs"
                >
                  <span className="text-tm-muted">[{e.entity_type}]</span>{" "}
                  <span className="text-tm-text">{e.name}</span>
                </span>
              ))}
            </div>
          </div>

          {/* Typed triples */}
          {result.typed_triples.length > 0 && (
            <div>
              <p className="text-xs text-tm-muted uppercase tracking-wider mb-2">
                Typed Relationships
              </p>
              <div className="space-y-1">
                {result.typed_triples.map((t, i) => (
                  <div key={i} className="text-sm">
                    <span className="text-tm-text">{t.subject}</span>
                    <span className="mx-2 text-tm-accent font-mono text-xs">{t.predicate}</span>
                    <span className="text-tm-text">{t.object}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          <p className="text-xs text-tm-muted">
            + {result.co_occurrence_count} co-occurrence triples
          </p>
        </div>
      )}
    </div>
  );
}
