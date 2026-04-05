import { useEffect, useState } from "react";
import { getTraces, type TraceInfo } from "../api";

export default function TracesView() {
  const [traces, setTraces] = useState<TraceInfo[]>([]);
  const [selected, setSelected] = useState<TraceInfo | null>(null);
  const [error, setError] = useState("");

  const refresh = () => {
    getTraces(100)
      .then(setTraces)
      .catch((e) => setError(String(e)));
  };

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  if (error) {
    return <div className="text-tm-red p-4">Error: {error}</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Audit Trail</h2>
        <button
          onClick={refresh}
          className="text-xs px-3 py-1.5 rounded bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text transition-colors"
        >
          Refresh
        </button>
      </div>

      <div className="flex gap-4 h-[calc(100vh-140px)]">
        {/* Trace list */}
        <div className="w-1/2 bg-tm-surface border border-tm-border rounded-lg overflow-y-auto">
          {traces.length === 0 ? (
            <p className="p-4 text-sm text-tm-muted">No traces recorded yet.</p>
          ) : (
            traces.map((trace) => (
              <button
                key={trace.id}
                onClick={() => setSelected(trace)}
                className={`w-full text-left p-4 border-b border-tm-border hover:bg-white/5 transition-colors ${
                  selected?.id === trace.id ? "bg-tm-accent/10 border-l-2 border-l-tm-accent" : ""
                }`}
              >
                <div className="flex items-center gap-2 mb-1">
                  <span
                    className={`text-xs font-mono px-1.5 py-0.5 rounded ${
                      trace.event_type === "Ingest"
                        ? "bg-tm-accent/20 text-tm-accent"
                        : "bg-tm-accent2/20 text-tm-accent2"
                    }`}
                  >
                    {trace.event_type}
                  </span>
                  <span className="text-xs text-tm-muted font-mono">{trace.id.slice(0, 8)}</span>
                  <span className="text-xs text-tm-muted ml-auto">{trace.created_at}</span>
                </div>
                <p className="text-sm truncate text-tm-text">
                  {trace.raw_text || "(no text)"}
                </p>
                <div className="flex gap-3 mt-1 text-xs text-tm-muted">
                  <span>{trace.entities_count} entities</span>
                  <span>{trace.triples_count} triples</span>
                  {trace.retrieval_latency_ms !== null && (
                    <span>{trace.retrieval_latency_ms}ms</span>
                  )}
                </div>
              </button>
            ))
          )}
        </div>

        {/* Trace detail */}
        <div className="w-1/2 bg-tm-surface border border-tm-border rounded-lg p-5 overflow-y-auto">
          {selected ? (
            <div className="space-y-4">
              <div className="flex items-center gap-2">
                <span
                  className={`text-xs font-mono px-2 py-1 rounded ${
                    selected.event_type === "Ingest"
                      ? "bg-tm-accent/20 text-tm-accent"
                      : "bg-tm-accent2/20 text-tm-accent2"
                  }`}
                >
                  {selected.event_type}
                </span>
                <h3 className="text-sm font-medium font-mono">{selected.id}</h3>
              </div>

              <div className="space-y-3">
                <DetailRow label="Created" value={selected.created_at} />
                <DetailRow label="Entities" value={String(selected.entities_count)} />
                <DetailRow label="Triples" value={String(selected.triples_count)} />

                {selected.retrieval_arm_name && (
                  <DetailRow
                    label="Strategy"
                    value={`${selected.retrieval_arm_name} (arm ${selected.retrieval_arm})`}
                  />
                )}

                {selected.retrieval_latency_ms !== null && (
                  <DetailRow label="Latency" value={`${selected.retrieval_latency_ms}ms`} />
                )}
              </div>

              {selected.raw_text && (
                <div>
                  <p className="text-xs text-tm-muted uppercase tracking-wider mb-2">
                    Raw Text
                  </p>
                  <div className="bg-tm-bg border border-tm-border rounded p-3 text-sm whitespace-pre-wrap">
                    {selected.raw_text}
                  </div>
                </div>
              )}

              <div className="pt-3 border-t border-tm-border">
                <p className="text-xs text-tm-muted">
                  Full provenance chain: every entity and triple extracted from this event
                  is tracked by ID for complete auditability. No data leaves your device.
                </p>
              </div>
            </div>
          ) : (
            <div className="flex items-center justify-center h-full text-sm text-tm-muted">
              Select a trace to view details
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-3">
      <span className="text-xs text-tm-muted w-20">{label}</span>
      <span className="text-sm text-tm-text">{value}</span>
    </div>
  );
}
