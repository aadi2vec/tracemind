// Daily brief panel — D-4 from the demo punch list.
//
// Renders the same DailyBrief the CLI shows, with one key visual: the
// contradictions row at the top is the demo's retraction-beat. Click
// row → drawer (still TODO; for now we render the pair inline with the
// short-ids and cosine score).

import { useEffect, useState } from "react";
import { getBrief, type BriefView as BriefData } from "../api";

function shortId(id: string): string {
  return id.slice(0, 8);
}

export default function BriefView() {
  const [brief, setBrief] = useState<BriefData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await getBrief();
      setBrief(data);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  if (error) {
    return (
      <div className="text-tm-muted">
        <p className="text-sm">Failed to load brief: {error}</p>
        <button
          onClick={refresh}
          className="mt-3 px-3 py-1.5 text-sm bg-tm-accent/10 text-tm-accent border border-tm-accent/30 rounded"
        >
          Retry
        </button>
      </div>
    );
  }

  if (!brief) {
    return <div className="text-tm-muted text-sm">{loading ? "Loading…" : ""}</div>;
  }

  const c = brief.counts;

  return (
    <div className="max-w-4xl">
      <div className="flex items-baseline justify-between mb-5">
        <div>
          <h2 className="text-lg font-semibold text-tm-text">TraceMind Brief</h2>
          <p className="text-xs text-tm-muted mt-0.5">
            generated {brief.generated_at}
          </p>
        </div>
        <button
          onClick={refresh}
          disabled={loading}
          className="px-3 py-1.5 text-xs bg-tm-accent/10 text-tm-accent border border-tm-accent/30 rounded hover:bg-tm-accent/20"
        >
          {loading ? "refreshing…" : "refresh"}
        </button>
      </div>

      <div className="flex flex-wrap gap-x-5 gap-y-1 text-xs text-tm-muted mb-6">
        <span>overdue: <span className="text-tm-text font-mono">{c.overdue}</span></span>
        <span>open: <span className="text-tm-text font-mono">{c.open}</span></span>
        <span>resolved: <span className="text-tm-text font-mono">{c.resolved}</span></span>
        <span>candidates: <span className="text-tm-text font-mono">{c.candidates}</span></span>
        <span>patterns: <span className="text-tm-text font-mono">{c.patterns}</span></span>
        <span>insights: <span className="text-tm-text font-mono">{c.insights}</span></span>
        <span>proposals: <span className="text-tm-text font-mono">{c.proposals}</span></span>
        <span className={c.contradictions > 0 ? "text-amber-400" : ""}>
          contradictions: <span className="font-mono">{c.contradictions}</span>
        </span>
      </div>

      {brief.contradictions.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-amber-400 mb-2">
            ⚡ contradictions ({brief.contradictions.length})
          </h3>
          <ul className="space-y-2">
            {brief.contradictions.map((row) => (
              <li
                key={row.id}
                className="border border-amber-500/30 bg-amber-500/5 rounded px-4 py-2.5 text-sm"
              >
                <div className="font-mono text-xs text-tm-muted">
                  {shortId(row.triple_a)} ↔ {shortId(row.triple_b)}
                </div>
                <div className="text-tm-text mt-1">
                  cosine{" "}
                  <span className="font-mono">
                    {row.cosine_similarity >= 0 ? "+" : ""}
                    {row.cosine_similarity.toFixed(2)}
                  </span>
                  {" · detected "}
                  <span className="text-tm-muted">{row.detected_at}</span>
                </div>
              </li>
            ))}
          </ul>
        </section>
      )}

      {brief.overdue.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            overdue ({brief.overdue.length})
          </h3>
          <ul className="space-y-1">
            {brief.overdue.map((r) => (
              <li
                key={r.id}
                className="flex items-baseline gap-3 text-sm border-b border-tm-border py-2"
              >
                <span className="font-mono text-xs text-tm-muted">
                  {shortId(r.id)}
                </span>
                <span className="text-xs text-amber-400 uppercase tracking-wide">
                  {r.overdue_class ?? "overdue"}
                </span>
                <span className="text-tm-text">{r.title}</span>
                {r.horizon && (
                  <span className="ml-auto text-xs text-tm-muted">{r.horizon}</span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}

      {brief.open.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            open ({brief.open.length})
          </h3>
          <ul className="space-y-1">
            {brief.open.map((r) => (
              <li
                key={r.id}
                className="flex items-baseline gap-3 text-sm border-b border-tm-border py-2"
              >
                <span className="font-mono text-xs text-tm-muted">
                  {shortId(r.id)}
                </span>
                <span className="text-tm-text">{r.title}</span>
                {r.horizon && (
                  <span className="ml-auto text-xs text-tm-muted">{r.horizon}</span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}

      {brief.resolved.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            resolved ({brief.resolved.length})
          </h3>
          <ul className="space-y-1">
            {brief.resolved.map((r) => (
              <li
                key={r.id}
                className="flex items-baseline gap-3 text-sm border-b border-tm-border py-2"
              >
                <span className="font-mono text-xs text-tm-muted">
                  {shortId(r.id)}
                </span>
                {r.polarity && (
                  <span
                    className={`text-xs uppercase tracking-wide ${
                      r.polarity === "Positive"
                        ? "text-emerald-400"
                        : r.polarity === "Negative"
                        ? "text-rose-400"
                        : "text-tm-muted"
                    }`}
                  >
                    {r.polarity}
                  </span>
                )}
                <span className="text-tm-text">{r.title}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {brief.overdue.length === 0 &&
        brief.open.length === 0 &&
        brief.resolved.length === 0 &&
        brief.contradictions.length === 0 && (
          <p className="text-sm text-tm-muted">
            No commitments yet. Run <code>tracemind demo restore</code> to seed
            the deterministic fixture, or capture some text to populate the
            graph.
          </p>
        )}
    </div>
  );
}
