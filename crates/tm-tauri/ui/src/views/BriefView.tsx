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

import { useEffect, useMemo, useState } from "react";
import {
  currentContext,
  getBrief,
  getTripleDetail,
  resolveContradiction,
  recordOutcome,
  type BriefView as BriefData,
  type ContextInfo,
  type ContradictionRow,
  type BriefRow,
  type ResolveChoice,
  type TripleDetailView,
  type OutcomePolarity,
} from "../api";
import EntityDrawer from "./EntityDrawer";
import TransclusionText from "./TransclusionText";

function shortId(id: string): string {
  return id.slice(0, 8);
}

// UI-9 — read / dismissed / archived state lives in localStorage so it
// survives reloads without touching the backend. Keys are scoped per
// brief row id; "read" is auto-marked when the user opens a drawer,
// "dismissed" hides a row from the active panel, "archived" stashes it
// in a collapsed section the user can re-open.
const LS_READ = "tm.brief.read";
const LS_DISMISSED = "tm.brief.dismissed";
const LS_ARCHIVED = "tm.brief.archived";

function loadSet(key: string): Set<string> {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return new Set();
    const arr = JSON.parse(raw) as string[];
    return new Set(Array.isArray(arr) ? arr : []);
  } catch {
    return new Set();
  }
}

function saveSet(key: string, set: Set<string>) {
  try {
    localStorage.setItem(key, JSON.stringify([...set]));
  } catch {
    /* quota / privacy mode — best-effort only */
  }
}

export default function BriefView() {
  const [brief, setBrief] = useState<BriefData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // C-0.9 — show the active context inline so the brief makes it
  // obvious which slice of the world model these commitments came from.
  const [activeContext, setActiveContext] = useState<ContextInfo | null>(null);

  // Drawer state: only one drawer can be open at a time.
  const [contradictionDrawer, setContradictionDrawer] =
    useState<ContradictionRow | null>(null);
  const [outcomeDrawer, setOutcomeDrawer] = useState<BriefRow | null>(null);
  // LM-3 — entity drawer opened from a transclusion chip click.
  const [entityDrawer, setEntityDrawer] = useState<string | null>(null);

  // UI-9 — read / dismissed / archived markers (localStorage).
  const [readIds, setReadIds] = useState<Set<string>>(() => loadSet(LS_READ));
  const [dismissedIds, setDismissedIds] = useState<Set<string>>(() =>
    loadSet(LS_DISMISSED),
  );
  const [archivedIds, setArchivedIds] = useState<Set<string>>(() =>
    loadSet(LS_ARCHIVED),
  );
  const [showArchived, setShowArchived] = useState(false);

  const markRead = (id: string) => {
    setReadIds((prev) => {
      if (prev.has(id)) return prev;
      const next = new Set(prev);
      next.add(id);
      saveSet(LS_READ, next);
      return next;
    });
  };

  const dismiss = (id: string) => {
    setDismissedIds((prev) => {
      const next = new Set(prev);
      next.add(id);
      saveSet(LS_DISMISSED, next);
      return next;
    });
  };

  const archive = (id: string) => {
    setArchivedIds((prev) => {
      const next = new Set(prev);
      next.add(id);
      saveSet(LS_ARCHIVED, next);
      return next;
    });
  };

  const unarchive = (id: string) => {
    setArchivedIds((prev) => {
      const next = new Set(prev);
      next.delete(id);
      saveSet(LS_ARCHIVED, next);
      return next;
    });
  };

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      const [data, ctx] = await Promise.all([
        getBrief(),
        currentContext().catch(() => null),
      ]);
      setBrief(data);
      setActiveContext(ctx);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  // Compute archived rows unconditionally so hook order is stable across renders.
  const archivedRows = useMemo(() => {
    if (!brief) return [] as BriefRow[];
    const all = [...brief.overdue, ...brief.open, ...brief.resolved];
    return all.filter((r) => archivedIds.has(r.id));
  }, [brief, archivedIds]);

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

  // UI-9 — filter dismissed/archived rows out of the active sections.
  // We render archived rows in a separate collapsible block so users
  // can recover them without touching the database.
  const visibleOverdue = brief.overdue.filter(
    (r) => !dismissedIds.has(r.id) && !archivedIds.has(r.id),
  );
  const visibleOpen = brief.open.filter(
    (r) => !dismissedIds.has(r.id) && !archivedIds.has(r.id),
  );
  const visibleResolved = brief.resolved.filter(
    (r) => !dismissedIds.has(r.id) && !archivedIds.has(r.id),
  );

  return (
    <div className="max-w-4xl">
      <div className="flex items-baseline justify-between mb-5">
        <div>
          <h2 className="text-lg font-semibold text-tm-text">TraceMind Brief</h2>
          <p className="text-xs text-tm-muted mt-0.5">
            generated {brief.generated_at}
            {activeContext && (
              <>
                {" · "}
                <span className="text-tm-muted">scope</span>{" "}
                <span className="text-tm-accent">{activeContext.name}</span>
              </>
            )}
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

      {brief.contradictions.filter((r) => !dismissedIds.has(r.id)).length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-amber-400 mb-2">
            ⚡ contradictions ({brief.contradictions.filter((r) => !dismissedIds.has(r.id)).length})
          </h3>
          <ul className="space-y-2">
            {brief.contradictions
              .filter((r) => !dismissedIds.has(r.id))
              .map((row) => (
                <li
                  key={row.id}
                  className="group border border-amber-500/30 bg-amber-500/5 rounded px-4 py-2.5 text-sm flex items-start gap-3"
                >
                  <button
                    onClick={() => {
                      markRead(row.id);
                      setContradictionDrawer(row);
                    }}
                    className="flex-1 text-left cursor-pointer"
                  >
                    <div className="font-mono text-xs text-tm-muted">
                      {!readIds.has(row.id) && (
                        <span className="inline-block w-1.5 h-1.5 rounded-full bg-amber-400 mr-1.5 align-middle" />
                      )}
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
                  </button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      dismiss(row.id);
                    }}
                    title="dismiss"
                    className="opacity-0 group-hover:opacity-100 text-tm-muted hover:text-rose-400 text-xs transition"
                  >
                    ×
                  </button>
                </li>
              ))}
          </ul>
        </section>
      )}

      {visibleOverdue.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            overdue ({visibleOverdue.length})
          </h3>
          <ul className="space-y-1">
            {visibleOverdue.map((r) => (
              <BriefRowItem
                key={r.id}
                row={r}
                read={readIds.has(r.id)}
                onOpen={() => {
                  markRead(r.id);
                  setOutcomeDrawer(r);
                }}
                onDismiss={() => dismiss(r.id)}
                onArchive={() => archive(r.id)}
                onOpenEntity={(id) => setEntityDrawer(id)}
                accent="overdue"
              />
            ))}
          </ul>
        </section>
      )}

      {visibleOpen.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            open ({visibleOpen.length})
          </h3>
          <ul className="space-y-1">
            {visibleOpen.map((r) => (
              <BriefRowItem
                key={r.id}
                row={r}
                read={readIds.has(r.id)}
                onOpen={() => {
                  markRead(r.id);
                  setOutcomeDrawer(r);
                }}
                onDismiss={() => dismiss(r.id)}
                onArchive={() => archive(r.id)}
                onOpenEntity={(id) => setEntityDrawer(id)}
              />
            ))}
          </ul>
        </section>
      )}

      {visibleResolved.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-tm-text mb-2">
            resolved ({visibleResolved.length})
          </h3>
          <ul className="space-y-1">
            {visibleResolved.map((r) => (
              <BriefRowItem
                key={r.id}
                row={r}
                read={readIds.has(r.id)}
                onOpen={() => markRead(r.id)}
                onDismiss={() => dismiss(r.id)}
                onArchive={() => archive(r.id)}
                onOpenEntity={(id) => setEntityDrawer(id)}
                accent="resolved"
              />
            ))}
          </ul>
        </section>
      )}

      {archivedRows.length > 0 && (
        <section className="mb-6">
          <button
            onClick={() => setShowArchived((v) => !v)}
            className="text-xs uppercase tracking-wider text-tm-muted hover:text-tm-text transition mb-2"
          >
            {showArchived ? "▾" : "▸"} archived ({archivedRows.length})
          </button>
          {showArchived && (
            <ul className="space-y-1 opacity-70">
              {archivedRows.map((r) => (
                <li
                  key={r.id}
                  className="flex items-baseline gap-3 text-sm border-b border-tm-border/40 py-1.5"
                >
                  <span className="font-mono text-xs text-tm-muted">
                    {shortId(r.id)}
                  </span>
                  <span className="text-tm-muted line-through decoration-tm-border">
                    {r.title}
                  </span>
                  <button
                    onClick={() => unarchive(r.id)}
                    className="ml-auto text-xs text-tm-muted hover:text-tm-accent transition"
                  >
                    restore
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      {visibleOverdue.length === 0 &&
        visibleOpen.length === 0 &&
        visibleResolved.length === 0 &&
        brief.contradictions.filter((r) => !dismissedIds.has(r.id)).length === 0 && (
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

      {entityDrawer && (
        <EntityDrawer
          entityId={entityDrawer}
          onClose={() => setEntityDrawer(null)}
          onOpenEntity={(t) => setEntityDrawer(t)}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// BriefRowItem — single row in the overdue/open/resolved lists. Carries
// the read-marker dot, hover-revealed dismiss/archive buttons, and the
// click-to-open-drawer affordance. UI-9 polish.
// ---------------------------------------------------------------------------

function BriefRowItem({
  row,
  read,
  onOpen,
  onDismiss,
  onArchive,
  onOpenEntity,
  accent,
}: {
  row: BriefRow;
  read: boolean;
  onOpen: () => void;
  onDismiss: () => void;
  onArchive: () => void;
  onOpenEntity?: (id: string) => void;
  accent?: "overdue" | "resolved";
}) {
  return (
    <li className="group flex items-baseline gap-3 text-sm border-b border-tm-border py-2 px-2 -mx-2 rounded hover:bg-tm-accent/5 transition">
      <button onClick={onOpen} className="flex items-baseline gap-3 flex-1 text-left">
        <span className="font-mono text-xs text-tm-muted flex items-center gap-1.5">
          {!read && (
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-tm-accent" />
          )}
          {shortId(row.id)}
        </span>
        {accent === "overdue" && (
          <span className="text-xs text-amber-400 uppercase tracking-wide">
            {row.overdue_class ?? "overdue"}
          </span>
        )}
        {accent === "resolved" && row.polarity && (
          <span
            className={`text-xs uppercase tracking-wide ${
              row.polarity === "Positive"
                ? "text-emerald-400"
                : row.polarity === "Negative"
                  ? "text-rose-400"
                  : "text-tm-muted"
            }`}
          >
            {row.polarity}
          </span>
        )}
        <TransclusionText
          text={row.title}
          onOpenEntity={onOpenEntity}
          className={read ? "text-tm-muted" : "text-tm-text"}
        />
        {row.horizon && (
          <span className="ml-auto text-xs text-tm-muted">{row.horizon}</span>
        )}
      </button>
      <div className="opacity-0 group-hover:opacity-100 flex items-center gap-1 transition">
        <button
          onClick={(e) => {
            e.stopPropagation();
            onArchive();
          }}
          title="archive"
          className="text-xs text-tm-muted hover:text-tm-accent"
        >
          ⌫
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDismiss();
          }}
          title="dismiss"
          className="text-xs text-tm-muted hover:text-rose-400"
        >
          ×
        </button>
      </div>
    </li>
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
