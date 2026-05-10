// Daily brief panel — D-4 + E series.
//
// Renders the same DailyBrief the CLI shows, plus two interactive
// drawers that the demo recording walks through:
//
//   • Contradiction drawer (Shot 2): click a contradiction row →
//     hydrate both triples (subject/predicate/object names + ingest
//     date) → user picks keep_a / keep_b / keep_both → brief refreshes
//     and the row leaves the contradictions section.
//
//   • Outcome-prompt drawer (Shot 4): click an overdue commitment →
//     "Did you ship it? What was the outcome?" → user picks polarity
//     → brief refreshes, the row moves from overdue → resolved.

import { useEffect, useState } from "react";
import {
  getBrief,
  getTripleDetail,
  resolveContradiction,
  recordOutcome,
  type BriefView as BriefData,
  type ContradictionRow,
  type BriefRow,
  type ResolveChoice,
  type TripleDetailView,
  type OutcomePolarity,
} from "../api";

function shortId(id: string): string {
  return id.slice(0, 8);
}

export default function BriefView() {
  const [brief, setBrief] = useState<BriefData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Drawer state: only one drawer can be open at a time.
  const [contradictionDrawer, setContradictionDrawer] =
    useState<ContradictionRow | null>(null);
  const [outcomeDrawer, setOutcomeDrawer] = useState<BriefRow | null>(null);

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
                onClick={() => setContradictionDrawer(row)}
                className="border border-amber-500/30 bg-amber-500/5 rounded px-4 py-2.5 text-sm cursor-pointer hover:bg-amber-500/10 transition"
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
                  <span className="text-amber-400 ml-2">→ review</span>
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
                onClick={() => setOutcomeDrawer(r)}
                className="flex items-baseline gap-3 text-sm border-b border-tm-border py-2 cursor-pointer hover:bg-tm-accent/5 px-2 -mx-2 rounded transition"
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
                onClick={() => setOutcomeDrawer(r)}
                className="flex items-baseline gap-3 text-sm border-b border-tm-border py-2 cursor-pointer hover:bg-tm-accent/5 px-2 -mx-2 rounded transition"
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

      {contradictionDrawer && (
        <ContradictionDrawer
          row={contradictionDrawer}
          onClose={() => setContradictionDrawer(null)}
          onResolved={async () => {
            setContradictionDrawer(null);
            await refresh();
          }}
        />
      )}

      {outcomeDrawer && (
        <OutcomeDrawer
          row={outcomeDrawer}
          onClose={() => setOutcomeDrawer(null)}
          onRecorded={async () => {
            setOutcomeDrawer(null);
            await refresh();
          }}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Contradiction drawer — Shot 2 of the demo.
//
// Loads both triples' detail (names + ingest dates), shows them side by
// side, and offers the three resolve choices.
// ---------------------------------------------------------------------------

function ContradictionDrawer({
  row,
  onClose,
  onResolved,
}: {
  row: ContradictionRow;
  onClose: () => void;
  onResolved: () => void;
}) {
  const [tripleA, setTripleA] = useState<TripleDetailView | null>(null);
  const [tripleB, setTripleB] = useState<TripleDetailView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [a, b] = await Promise.all([
          getTripleDetail(row.triple_a),
          getTripleDetail(row.triple_b),
        ]);
        if (!cancelled) {
          setTripleA(a);
          setTripleB(b);
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [row.triple_a, row.triple_b]);

  const choose = async (choice: ResolveChoice) => {
    setBusy(true);
    setError(null);
    try {
      await resolveContradiction(row.triple_a, row.triple_b, choice);
      onResolved();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <DrawerShell title="Contradiction" onClose={onClose}>
      {error && (
        <p className="text-sm text-rose-400 mb-3">Failed: {error}</p>
      )}
      <div className="space-y-3">
        <TripleCard label="A" detail={tripleA} fallbackId={row.triple_a} />
        <div className="text-center text-xs text-amber-400 uppercase tracking-wider">
          vs (cosine {row.cosine_similarity >= 0 ? "+" : ""}
          {row.cosine_similarity.toFixed(2)})
        </div>
        <TripleCard label="B" detail={tripleB} fallbackId={row.triple_b} />
      </div>

      <div className="mt-5 grid grid-cols-3 gap-2">
        <button
          disabled={busy}
          onClick={() => choose("keep_a")}
          className="px-3 py-2 text-sm border border-emerald-500/40 bg-emerald-500/10 text-emerald-300 rounded hover:bg-emerald-500/20 disabled:opacity-50"
        >
          keep A
        </button>
        <button
          disabled={busy}
          onClick={() => choose("keep_b")}
          className="px-3 py-2 text-sm border border-emerald-500/40 bg-emerald-500/10 text-emerald-300 rounded hover:bg-emerald-500/20 disabled:opacity-50"
        >
          keep B
        </button>
        <button
          disabled={busy}
          onClick={() => choose("keep_both")}
          className="px-3 py-2 text-sm border border-tm-accent/40 bg-tm-accent/10 text-tm-accent rounded hover:bg-tm-accent/20 disabled:opacity-50"
        >
          keep both
        </button>
      </div>
      <p className="text-xs text-tm-muted mt-3 leading-relaxed">
        keep both = "they're about different times". Neither triple is
        retracted; the contradiction is marked resolved so the brief stops
        surfacing it.
      </p>
    </DrawerShell>
  );
}

function TripleCard({
  label,
  detail,
  fallbackId,
}: {
  label: string;
  detail: TripleDetailView | null;
  fallbackId: string;
}) {
  if (!detail) {
    return (
      <div className="border border-tm-border rounded p-3 text-sm text-tm-muted">
        <span className="text-xs uppercase tracking-wide mr-2">{label}</span>
        loading {shortId(fallbackId)}…
      </div>
    );
  }
  return (
    <div className="border border-tm-border rounded p-3 text-sm">
      <div className="flex items-center gap-2 mb-1.5">
        <span className="text-xs uppercase tracking-wide text-tm-muted">
          {label}
        </span>
        {detail.status && (
          <span
            className={`text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded ${
              detail.status === "Out"
                ? "bg-rose-500/15 text-rose-300"
                : detail.status === "Contradicted"
                ? "bg-amber-500/15 text-amber-300"
                : "bg-emerald-500/15 text-emerald-300"
            }`}
          >
            {detail.status}
          </span>
        )}
      </div>
      <div className="text-tm-text">
        <span className="font-medium">{detail.subject_name}</span>
        <span className="text-tm-muted mx-2">{detail.predicate}</span>
        <span className="font-medium">{detail.object_name}</span>
      </div>
      <div className="text-xs text-tm-muted mt-1.5">
        ingested {detail.ingested_at}
        {detail.source_id && (
          <>
            {" · "}from <span className="font-mono">{shortId(detail.source_id)}</span>
          </>
        )}
        {" · "}confidence{" "}
        <span className="font-mono">{detail.confidence.toFixed(2)}</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Outcome-prompt drawer — Shot 4 of the demo.
//
// The brief shows an overdue commitment; click → drawer asks "Did you
// ship it? What was the outcome?" The polarity buttons map to
// `tm_intent::Polarity` and complete the commitment.
// ---------------------------------------------------------------------------

function OutcomeDrawer({
  row,
  onClose,
  onRecorded,
}: {
  row: BriefRow;
  onClose: () => void;
  onRecorded: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState("");

  const choose = async (polarity: OutcomePolarity) => {
    setBusy(true);
    setError(null);
    try {
      await recordOutcome(row.id, polarity, undefined, note || undefined);
      onRecorded();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <DrawerShell title="Did you ship it? What was the outcome?" onClose={onClose}>
      {error && <p className="text-sm text-rose-400 mb-3">Failed: {error}</p>}
      <div className="text-sm text-tm-text mb-1">{row.title}</div>
      <div className="text-xs text-tm-muted mb-4">
        {row.horizon && <>due {row.horizon} · </>}
        commitment <span className="font-mono">{shortId(row.id)}</span>
      </div>

      <textarea
        value={note}
        onChange={(e) => setNote(e.target.value)}
        placeholder="optional note — what actually happened?"
        rows={3}
        className="w-full text-sm bg-tm-bg border border-tm-border rounded px-2 py-1.5 mb-4 placeholder-tm-muted focus:outline-none focus:border-tm-accent"
      />

      <div className="grid grid-cols-3 gap-2">
        <PolarityBtn
          disabled={busy}
          onClick={() => choose("better")}
          className="border-emerald-500/40 bg-emerald-500/10 text-emerald-300 hover:bg-emerald-500/20"
        >
          completed · positive
        </PolarityBtn>
        <PolarityBtn
          disabled={busy}
          onClick={() => choose("as_expected")}
          className="border-tm-border bg-tm-accent/5 text-tm-text hover:bg-tm-accent/10"
        >
          completed · neutral
        </PolarityBtn>
        <PolarityBtn
          disabled={busy}
          onClick={() => choose("worse")}
          className="border-rose-500/40 bg-rose-500/10 text-rose-300 hover:bg-rose-500/20"
        >
          completed · negative
        </PolarityBtn>
      </div>
    </DrawerShell>
  );
}

function PolarityBtn({
  children,
  className,
  disabled,
  onClick,
}: {
  children: React.ReactNode;
  className: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      className={`px-3 py-2 text-xs uppercase tracking-wide border rounded transition disabled:opacity-50 ${className}`}
    >
      {children}
    </button>
  );
}

// ---------------------------------------------------------------------------
// DrawerShell — minimalist right-aligned panel. Click overlay to close.
// ---------------------------------------------------------------------------

function DrawerShell({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className="fixed inset-0 z-50 flex">
      <div
        onClick={onClose}
        className="flex-1 bg-black/40 backdrop-blur-sm"
        aria-hidden
      />
      <div className="w-full max-w-md bg-tm-bg border-l border-tm-border p-5 overflow-y-auto">
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-sm font-semibold text-tm-text">{title}</h3>
          <button
            onClick={onClose}
            className="text-tm-muted hover:text-tm-text text-sm"
            aria-label="close"
          >
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
