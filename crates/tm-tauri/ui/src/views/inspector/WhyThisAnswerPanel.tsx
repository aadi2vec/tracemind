import { useEffect, useMemo, useState } from "react";
import { TraceInfo, WhyTraceView, getTraceWhy, getTraces } from "../../api";

// Panel 2 — "Why this answer?"
// User picks a trace from the dropdown (newest first, default to first
// Retrieve trace). We call cmd_trace_why and render:
//   - planner classification (action / complexity / confidence / hints)
//   - per-arm LinUCB scores (exploit + α·explore) with current LinUCB state
//   - entities the trace returned (resolved to current names)
// No mocks — score recomputation is real and uses `engine.embed_query()` +
// the persisted `linucb.json`.

function fmtTimestamp(t: string): string {
  const d = new Date(t);
  if (Number.isNaN(d.getTime())) return t;
  return d.toLocaleString();
}

export default function WhyThisAnswerPanel() {
  const [traces, setTraces] = useState<TraceInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [why, setWhy] = useState<WhyTraceView | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [listLoading, setListLoading] = useState(true);

  async function loadList() {
    setListLoading(true);
    try {
      const list = await getTraces(200);
      setTraces(list);
      // Prefer the most-recent Retrieve trace, fall back to the most recent.
      const first =
        list.find((t) => t.event_type === "Retrieve") || list[0] || null;
      if (first) setSelectedId(first.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setListLoading(false);
    }
  }

  useEffect(() => {
    loadList();
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setWhy(null);
      return;
    }
    setLoading(true);
    setError(null);
    getTraceWhy(selectedId)
      .then((v) => setWhy(v))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [selectedId]);

  const selectedTrace = useMemo(
    () => traces.find((t) => t.id === selectedId) || null,
    [traces, selectedId],
  );

  return (
    <div className="space-y-5">
      {/* Trace picker */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <h3 className="text-base font-medium text-tm-text mb-1">Trace</h3>
        <p className="text-xs text-tm-muted mb-3">
          {listLoading
            ? "Loading recent traces…"
            : `Showing ${traces.length.toLocaleString()} traces (newest first).`}
        </p>
        <select
          value={selectedId ?? ""}
          onChange={(e) => setSelectedId(e.target.value || null)}
          className="w-full bg-black/30 border border-tm-border rounded p-2 text-sm text-tm-text"
        >
          <option value="" disabled>
            — pick a trace —
          </option>
          {traces.map((t) => {
            const stamp = fmtTimestamp(t.created_at);
            const head = (t.raw_text || "").slice(0, 80).replace(/\s+/g, " ");
            return (
              <option key={t.id} value={t.id}>
                [{t.event_type}] {stamp}
                {t.retrieval_arm !== null && t.retrieval_arm !== undefined
                  ? ` · arm ${t.retrieval_arm}`
                  : ""}
                {head ? ` — ${head}` : ""}
              </option>
            );
          })}
        </select>
        {selectedTrace && (
          <p className="text-xs text-tm-muted mt-2">
            ID: <code className="text-tm-accent">{selectedTrace.id}</code>
          </p>
        )}
      </section>

      {error && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200">
          {error}
        </div>
      )}

      {loading && <p className="text-sm text-tm-muted">Recomputing…</p>}

      {why && !loading && (
        <>
          {/* Header */}
          <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <div className="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
              <KV label="Event type" value={why.event_type} />
              <KV label="Created at" value={fmtTimestamp(why.created_at)} />
              <KV
                label="Selected arm"
                value={
                  why.selected_arm !== null && why.selected_arm !== undefined
                    ? `${why.selected_arm} (${why.selected_arm_name || "?"})`
                    : "—"
                }
              />
              <KV
                label="Latency"
                value={why.latency_ms !== null && why.latency_ms !== undefined ? `${why.latency_ms} ms` : "—"}
              />
              <KV label="Governance gate" value={why.confidence_gate_passed ? "passed" : "BLOCKED"} />
              <KV label="LinUCB α (current)" value={why.linucb_alpha.toFixed(4)} />
            </div>
            {why.raw_text && (
              <div className="mt-4">
                <p className="text-[10px] uppercase tracking-wider text-tm-muted mb-1">Raw text</p>
                <pre className="text-xs whitespace-pre-wrap text-tm-text font-mono bg-black/30 border border-tm-border rounded p-2 max-h-40 overflow-y-auto">
                  {why.raw_text}
                </pre>
              </div>
            )}
          </section>

          {/* Planner classification */}
          {(why.plan_action || why.plan_complexity) && (
            <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <h3 className="text-base font-medium text-tm-text mb-3">Planner classification</h3>
              <div className="grid grid-cols-2 md:grid-cols-3 gap-3 text-sm">
                <KV label="Action" value={why.plan_action || "—"} />
                <KV label="Complexity" value={why.plan_complexity || "—"} />
                <KV
                  label="Confidence"
                  value={why.plan_confidence !== null ? why.plan_confidence!.toFixed(3) : "—"}
                />
              </div>
              {why.plan_entity_hints.length > 0 && (
                <div className="mt-3">
                  <p className="text-[10px] uppercase tracking-wider text-tm-muted mb-1">
                    Entity hints
                  </p>
                  <div className="flex flex-wrap gap-1.5">
                    {why.plan_entity_hints.map((h, i) => (
                      <span
                        key={i}
                        className="text-xs px-2 py-0.5 bg-tm-accent/10 border border-tm-accent/30 text-tm-accent rounded"
                      >
                        {h}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </section>
          )}

          {/* Per-arm LinUCB scores */}
          {why.arm_scores.length > 0 && (
            <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <h3 className="text-base font-medium text-tm-text">Per-arm LinUCB scores</h3>
              <p className="text-xs text-tm-muted mb-3">
                Re-computed against the <em>current</em> LinUCB state using
                the same query embedding. (Historical scores at decision time
                aren't persisted — these show what the model would do today.)
              </p>
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-left text-tm-muted border-b border-tm-border">
                      <th className="py-2 pr-3">Arm</th>
                      <th className="py-2 pr-3">Name</th>
                      <th className="py-2 pr-3">top_k</th>
                      <th className="py-2 pr-3">hops</th>
                      <th className="py-2 pr-3">ep</th>
                      <th className="py-2 pr-3">cb</th>
                      <th className="py-2 pr-3 text-right">exploit</th>
                      <th className="py-2 pr-3 text-right">explore</th>
                      <th className="py-2 pr-3 text-right">total</th>
                      <th className="py-2 pr-3 text-right">pulls</th>
                    </tr>
                  </thead>
                  <tbody>
                    {why.arm_scores
                      .slice()
                      .sort((a, b) => b.total_score - a.total_score)
                      .map((r) => {
                        const isSelected = r.arm === why.selected_arm;
                        return (
                          <tr
                            key={r.arm}
                            className={`border-b border-tm-border/40 ${
                              isSelected ? "bg-tm-accent/10" : ""
                            }`}
                          >
                            <td className="py-1.5 pr-3 text-tm-muted">{r.arm}</td>
                            <td className="py-1.5 pr-3 font-medium text-tm-text">
                              {r.name}
                              {isSelected && (
                                <span className="ml-2 text-[10px] uppercase tracking-wider text-tm-accent">
                                  picked
                                </span>
                              )}
                            </td>
                            <td className="py-1.5 pr-3 tabular-nums">{r.top_k}</td>
                            <td className="py-1.5 pr-3 tabular-nums">{r.hops}</td>
                            <td className="py-1.5 pr-3">{r.include_episodic ? "✓" : "·"}</td>
                            <td className="py-1.5 pr-3">{r.include_colbert ? "✓" : "·"}</td>
                            <td className="py-1.5 pr-3 text-right tabular-nums">{r.exploit_score.toFixed(4)}</td>
                            <td className="py-1.5 pr-3 text-right tabular-nums">{r.explore_score.toFixed(4)}</td>
                            <td className="py-1.5 pr-3 text-right tabular-nums font-medium">{r.total_score.toFixed(4)}</td>
                            <td className="py-1.5 pr-3 text-right tabular-nums">{r.pulls.toLocaleString()}</td>
                          </tr>
                        );
                      })}
                  </tbody>
                </table>
              </div>
            </section>
          )}

          {/* Entities */}
          {why.entities.length > 0 && (
            <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <h3 className="text-base font-medium text-tm-text mb-3">Entities returned</h3>
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-left text-tm-muted border-b border-tm-border">
                      <th className="py-2 pr-3">Name</th>
                      <th className="py-2 pr-3">Type</th>
                      <th className="py-2 pr-3 text-right">Confidence</th>
                      <th className="py-2 pr-3">ID</th>
                    </tr>
                  </thead>
                  <tbody>
                    {why.entities.map((e) => (
                      <tr key={e.entity_id} className="border-b border-tm-border/40">
                        <td className="py-1.5 pr-3 font-medium text-tm-text">{e.name}</td>
                        <td className="py-1.5 pr-3 text-tm-muted">{e.entity_type}</td>
                        <td className="py-1.5 pr-3 text-right tabular-nums">{e.confidence.toFixed(3)}</td>
                        <td className="py-1.5 pr-3">
                          <code className="text-xs text-tm-muted">{e.entity_id.slice(0, 8)}…</code>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          )}
        </>
      )}
    </div>
  );
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-black/20 border border-tm-border/60 rounded p-2.5">
      <p className="text-[10px] uppercase tracking-wider text-tm-muted">{label}</p>
      <p className="text-sm font-medium text-tm-text mt-0.5">{value}</p>
    </div>
  );
}
