import { useEffect, useState } from "react";
import { getBrief, type BriefRow, type BriefView } from "../api";

// UI-10 — vertical commitment timeline, color-coded by state.
// Pulls from cmd_brief (which already buckets commitments into
// overdue / open / resolved) and renders a single chronological
// list. Clicking a row toggles a drawer with its title + state
// metadata. Real outcome attachment lives in the Brief view; this
// is the "see the arc" surface.

type State = "overdue" | "open" | "resolved";

interface Row extends BriefRow {
  bucket: State;
}

function badgeFor(bucket: State): { label: string; color: string } {
  switch (bucket) {
    case "overdue":
      return { label: "Overdue", color: "bg-red-500/20 text-red-300 border-red-500/40" };
    case "open":
      return { label: "Open", color: "bg-amber-500/20 text-amber-300 border-amber-500/40" };
    case "resolved":
      return { label: "Resolved", color: "bg-green-500/20 text-green-300 border-green-500/40" };
  }
}

export default function CommitmentTimelineView() {
  const [brief, setBrief] = useState<BriefView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    getBrief()
      .then(setBrief)
      .catch((e) => setError(String(e)));
  }, []);

  const rows: Row[] = brief
    ? [
        ...brief.overdue.map((r) => ({ ...r, bucket: "overdue" as State })),
        ...brief.open.map((r) => ({ ...r, bucket: "open" as State })),
        ...brief.resolved.map((r) => ({ ...r, bucket: "resolved" as State })),
      ]
    : [];

  return (
    <div className="max-w-3xl mx-auto">
      <header className="mb-6">
        <h2 className="text-2xl font-semibold text-tm-text">Commitments</h2>
        <p className="text-sm text-tm-muted mt-1">
          Every "I'll …" / "by Friday" / "let me know when" TraceMind has noticed. Color-coded by
          state.
        </p>
      </header>

      {error && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200 mb-4">
          {error}
        </div>
      )}

      {brief && rows.length === 0 && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-6 text-center">
          <p className="text-sm text-tm-muted">No commitments tracked yet.</p>
          <p className="text-xs text-tm-muted mt-1">
            They'll show up once you ingest text containing soft promises (e.g. "I'll send the
            deck by Tuesday").
          </p>
        </div>
      )}

      {rows.length > 0 && (
        <div className="relative pl-8">
          <div className="absolute left-3 top-2 bottom-2 w-px bg-tm-border" />
          <ul className="space-y-3">
            {rows.map((r) => {
              const badge = badgeFor(r.bucket);
              const isOpen = expanded === r.id;
              return (
                <li key={r.id} className="relative">
                  <span
                    className={`absolute -left-[26px] top-2 w-3 h-3 rounded-full border-2 ${
                      r.bucket === "overdue"
                        ? "bg-red-500 border-red-500"
                        : r.bucket === "open"
                          ? "bg-amber-500 border-amber-500"
                          : "bg-green-500 border-green-500"
                    }`}
                  />
                  <button
                    onClick={() => setExpanded(isOpen ? null : r.id)}
                    className="w-full text-left bg-tm-surface border border-tm-border rounded p-3 hover:bg-white/5 transition-colors"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-sm font-medium text-tm-text truncate">{r.title}</span>
                      <span
                        className={`text-[10px] uppercase tracking-wider px-2 py-0.5 rounded border ${badge.color}`}
                      >
                        {badge.label}
                      </span>
                    </div>
                    {r.horizon && (
                      <p className="text-xs text-tm-muted mt-1">{r.horizon}</p>
                    )}
                    {isOpen && (
                      <div className="mt-3 pt-3 border-t border-tm-border/60 text-xs text-tm-muted space-y-1">
                        <p>
                          State: <span className="text-tm-text">{r.state}</span>
                        </p>
                        {r.polarity && (
                          <p>
                            Outcome: <span className="text-tm-text">{r.polarity}</span>
                          </p>
                        )}
                        {r.overdue_class && (
                          <p>
                            Overdue class: <span className="text-tm-text">{r.overdue_class}</span>
                          </p>
                        )}
                      </div>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
