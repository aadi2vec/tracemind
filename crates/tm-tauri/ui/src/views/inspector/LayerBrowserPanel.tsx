import { useEffect, useState } from "react";
import {
  InspectorLayerName,
  InspectorLayerPayload,
  getInspectorLayer,
} from "../../api";

// Panel 4 — Layer browser.
// Pick one of the named layers, render the raw JSON payload as a
// pretty-printed code block + a small summary banner. Each layer's
// payload shape is layer-specific — the panel doesn't normalize it.

const LAYERS: { id: InspectorLayerName; label: string; hint: string }[] = [
  { id: "vector", label: "Vector", hint: "BGE-small, 384d, cosine" },
  { id: "graph", label: "Graph", hint: "entities, triples, communities" },
  { id: "episodic", label: "Episodic", hint: "trace log + recent ring" },
  { id: "bandit", label: "Bandit", hint: "UCB1 + LinUCB (5 arms)" },
  { id: "governance", label: "Governance", hint: "PII regex + gate stats" },
  { id: "reason", label: "Reason", hint: "chains, analogies, contradictions" },
];

export default function LayerBrowserPanel() {
  const [layer, setLayer] = useState<InspectorLayerName>("vector");
  const [payload, setPayload] = useState<InspectorLayerPayload | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function load(l: InspectorLayerName) {
    setLoading(true);
    setError(null);
    try {
      setPayload(await getInspectorLayer(l));
    } catch (e) {
      setError(String(e));
      setPayload(null);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    load(layer);
  }, [layer]);

  const meta = LAYERS.find((l) => l.id === layer)!;

  // Pull a few top-level scalars out for a summary strip — the rest goes
  // into the raw JSON view.
  const headline = payload
    ? Object.entries(payload)
        .filter(
          ([k, v]) =>
            (typeof v === "number" || typeof v === "string" || typeof v === "boolean") &&
            !["layer"].includes(k),
        )
        .slice(0, 8)
    : [];

  return (
    <div className="space-y-5">
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <h3 className="text-base font-medium text-tm-text mb-1">Layer</h3>
        <p className="text-xs text-tm-muted mb-3">
          Switching layer re-fetches its raw state from the backend; nothing here is cached.
        </p>
        <div className="flex flex-wrap gap-2">
          {LAYERS.map((l) => (
            <button
              key={l.id}
              onClick={() => setLayer(l.id)}
              className={`px-3 py-1.5 text-sm rounded border transition-colors ${
                l.id === layer
                  ? "bg-tm-accent/10 border-tm-accent text-tm-accent"
                  : "bg-black/20 border-tm-border text-tm-muted hover:text-tm-text"
              }`}
              title={l.hint}
            >
              {l.label}
            </button>
          ))}
        </div>
      </section>

      {error && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200">
          {error}
        </div>
      )}

      {loading && <p className="text-sm text-tm-muted">Loading {layer} layer…</p>}

      {payload && !loading && (
        <>
          <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <div className="flex items-baseline justify-between mb-3">
              <div>
                <h3 className="text-base font-medium text-tm-text">{meta.label} — summary</h3>
                <p className="text-xs text-tm-muted">{meta.hint}</p>
              </div>
              <button
                onClick={() => load(layer)}
                className="text-xs px-2.5 py-1 border border-tm-border rounded text-tm-muted hover:text-tm-text"
              >
                ↻ resample
              </button>
            </div>
            {headline.length === 0 ? (
              <p className="text-sm text-tm-muted">No scalar headline metrics for this layer.</p>
            ) : (
              <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
                {headline.map(([k, v]) => (
                  <div key={k} className="bg-black/20 border border-tm-border/60 rounded p-2.5">
                    <p className="text-[10px] uppercase tracking-wider text-tm-muted truncate" title={k}>
                      {k}
                    </p>
                    <p className="text-sm font-medium text-tm-text mt-0.5 break-words">
                      {String(v)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <h3 className="text-base font-medium text-tm-text mb-3">Raw payload</h3>
            <pre className="text-xs font-mono text-tm-text bg-black/40 border border-tm-border rounded p-3 max-h-[480px] overflow-auto whitespace-pre">
              {JSON.stringify(payload, null, 2)}
            </pre>
          </section>
        </>
      )}
    </div>
  );
}
