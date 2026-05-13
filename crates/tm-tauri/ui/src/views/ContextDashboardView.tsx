// Context Dashboard — one screen, every brain layer for a single entity.
//
// The user feedback (May 2026) was: "the Graph view is only one part of our
// brain. When I ask about X I want every piece of context we have — not just
// edges." This view collapses ~12 panels (header, decay, graph in/out,
// vector neighbors, k-hop, community, belief, contradictions, bitemporal
// provenance, recent traces, signal neighbors) into one screen, all wired to
// the single `cmd_entity_context_dump` invoke.
//
// Decay is surfaced *first* by deliberate design — the user explicitly asked
// us to "make sure we get the full value of whatever we capture, including
// temporal decay". Recency / novelty / value / frequency are the four
// scoring axes the retrieval bandit already uses; exposing them per-entity
// gives the user a direct read on "what TraceMind still values".

import { useEffect, useMemo, useState } from "react";
import { getEntityContext, type EntityContextDump } from "../api";

function fmtPct(x: number): string {
  return `${(x * 100).toFixed(0)}%`;
}

function fmtNum(x: number, digits = 2): string {
  return x.toFixed(digits);
}

function shortId(s: string): string {
  return s.slice(0, 8);
}

function StatusPill({ status }: { status: string }) {
  const tone =
    status === "In"
      ? "bg-emerald-500/10 text-emerald-300 border-emerald-500/30"
      : status === "Contradicted"
        ? "bg-rose-500/10 text-rose-300 border-rose-500/30"
        : status === "Out"
          ? "bg-amber-500/10 text-amber-300 border-amber-500/30"
          : "bg-tm-surface/40 text-tm-muted border-tm-border";
  return (
    <span
      className={`text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded border ${tone}`}
    >
      {status}
    </span>
  );
}

/** Horizontal mini-bar for a 0..1 score. */
function ScoreBar({
  label,
  value,
  tone = "accent",
  hint,
}: {
  label: string;
  value: number;
  tone?: "accent" | "green" | "amber" | "rose";
  hint?: string;
}) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  const fill =
    tone === "green"
      ? "bg-emerald-400"
      : tone === "amber"
        ? "bg-amber-400"
        : tone === "rose"
          ? "bg-rose-400"
          : "bg-tm-accent";
  return (
    <div>
      <div className="flex items-baseline justify-between mb-1">
        <span className="text-xs text-tm-muted">{label}</span>
        <span className="text-xs font-mono text-tm-text">{fmtPct(value)}</span>
      </div>
      <div className="h-1.5 bg-tm-surface rounded overflow-hidden">
        <div className={`h-full ${fill}`} style={{ width: `${pct}%` }} />
      </div>
      {hint && (
        <p className="text-[10px] text-tm-muted mt-1 leading-tight">{hint}</p>
      )}
    </div>
  );
}

function Section({
  title,
  count,
  children,
  defaultOpen = true,
}: {
  title: string;
  count?: number;
  children: React.ReactNode;
  defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="border border-tm-border rounded mb-3 bg-tm-surface/20">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-baseline justify-between px-3 py-2 hover:bg-tm-surface/30 transition-colors"
      >
        <span className="text-sm font-semibold text-tm-text">
          {title}
          {typeof count === "number" && (
            <span className="ml-2 text-xs font-mono text-tm-muted">
              ({count})
            </span>
          )}
        </span>
        <span className="text-tm-muted text-xs">{open ? "▾" : "▸"}</span>
      </button>
      {open && <div className="px-3 pb-3 pt-1">{children}</div>}
    </section>
  );
}

export default function ContextDashboardView() {
  const [query, setQuery] = useState("");
  const [submitted, setSubmitted] = useState<string | null>(null);
  const [dump, setDump] = useState<EntityContextDump | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Honour the cross-view focus event so the Graph view can drop a user
  // into Context Dashboard for the clicked entity without page reload.
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent).detail as
        | { entity_id?: string; entity_name?: string }
        | undefined;
      const target = detail?.entity_id || detail?.entity_name;
      if (target) {
        setQuery(target);
        setSubmitted(target);
      }
    };
    window.addEventListener("tm:focus-entity", handler);
    return () => window.removeEventListener("tm:focus-entity", handler);
  }, []);

  useEffect(() => {
    if (!submitted) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    getEntityContext(submitted)
      .then((d) => {
        if (!cancelled) setDump(d);
      })
      .catch((e) => {
        if (!cancelled) {
          setError(String(e));
          setDump(null);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [submitted]);

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = query.trim();
    if (trimmed) setSubmitted(trimmed);
  };

  // Surface-level dispatcher: a click on a neighbor / sibling re-targets
  // the dashboard at that entity in place.
  const focusEntity = (target: string) => {
    setQuery(target);
    setSubmitted(target);
  };

  const accessHint = useMemo(() => {
    if (!dump) return "";
    return `Touched ${dump.decay.access_count} time${dump.decay.access_count === 1 ? "" : "s"}.`;
  }, [dump]);

  return (
    <div className="max-w-5xl">
      <div className="mb-5">
        <h2 className="text-lg font-semibold text-tm-text">Context</h2>
        <p className="text-xs text-tm-muted mt-0.5">
          One screen, every layer — graph, vectors, signals, beliefs, decay,
          provenance.
        </p>
      </div>

      <form onSubmit={onSubmit} className="flex gap-2 mb-5">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="entity name or UUID…"
          className="flex-1 px-3 py-2 text-sm bg-tm-surface/40 border border-tm-border rounded text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent"
        />
        <button
          type="submit"
          disabled={loading || !query.trim()}
          className="px-3 py-1.5 text-xs bg-tm-accent/10 text-tm-accent border border-tm-accent/30 rounded hover:bg-tm-accent/20 disabled:opacity-40"
        >
          {loading ? "loading…" : "load"}
        </button>
      </form>

      {error && (
        <p className="text-sm text-rose-400 mb-4">Failed: {error}</p>
      )}

      {!submitted && !dump && !error && (
        <p className="text-xs text-tm-muted">
          Enter an entity name (e.g. <code>TraceMind</code>) or paste a UUID to
          see everything we know.
        </p>
      )}

      {dump && (
        <>
          {/* ── Header ────────────────────────────────────────────────── */}
          <Section title="header">
            <div className="text-base font-semibold text-tm-text">
              {dump.header.name}
            </div>
            <div className="text-xs text-tm-muted mt-1 flex flex-wrap gap-x-3 gap-y-1">
              <span>
                type{" "}
                <span className="text-tm-text">
                  {dump.header.entity_type}
                </span>
              </span>
              <span>
                domain{" "}
                <span className="text-tm-accent">
                  {dump.header.ontological_domain}
                </span>
              </span>
              <span>
                confidence{" "}
                <span className="font-mono text-tm-text">
                  {fmtNum(dump.header.confidence)}
                </span>
              </span>
              <span className="font-mono">
                {shortId(dump.header.entity_id)}
              </span>
              <span>created {dump.header.created_at.slice(0, 10)}</span>
              <span>updated {dump.header.updated_at.slice(0, 10)}</span>
            </div>
          </Section>

          {/* ── Decay (highlighted, surfaced first) ────────────────────── */}
          <Section title="temporal decay">
            <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
              <ScoreBar
                label="recency"
                value={dump.decay.recency}
                tone={dump.decay.recency > 0.5 ? "green" : "amber"}
                hint={`Half-life ~14h since last touch. ${accessHint}`}
              />
              <ScoreBar
                label="novelty"
                value={dump.decay.novelty}
                tone={dump.decay.novelty > 0.5 ? "accent" : "amber"}
                hint="Drops as access count grows."
              />
              <ScoreBar
                label="value (feedback)"
                value={dump.decay.value}
                tone={dump.decay.value > 0.3 ? "green" : "amber"}
                hint="From recorded retrieval feedback."
              />
              <ScoreBar
                label="frequency"
                value={dump.decay.frequency}
                tone="accent"
                hint="Inverse of retrieval count."
              />
            </div>
          </Section>

          {/* ── Community ──────────────────────────────────────────────── */}
          <Section
            title="community"
            count={dump.community.sibling_count}
          >
            {dump.community.community_id === null ? (
              <p className="text-xs text-tm-muted">
                Not in any community yet — Louvain hasn't found a cohesive
                neighborhood around this entity.
              </p>
            ) : (
              <>
                <div className="flex items-baseline gap-2 mb-2 flex-wrap">
                  <span className="text-sm text-tm-text font-medium">
                    {dump.community.label || `c${dump.community.community_id}`}
                  </span>
                  <span className="text-xs font-mono text-tm-muted">
                    #{dump.community.community_id}
                  </span>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {dump.community.sibling_names.map((n) => (
                    <button
                      key={n}
                      onClick={() => focusEntity(n)}
                      className="text-xs px-2 py-0.5 rounded border border-tm-border bg-tm-surface/30 text-tm-text hover:border-tm-accent hover:text-tm-accent transition-colors"
                    >
                      {n}
                    </button>
                  ))}
                </div>
              </>
            )}
          </Section>

          {/* ── Graph: outgoing relations ──────────────────────────────── */}
          <Section
            title="outgoing relations"
            count={dump.relations_out.length}
          >
            {dump.relations_out.length === 0 ? (
              <p className="text-xs text-tm-muted">none</p>
            ) : (
              <ul className="space-y-1">
                {dump.relations_out.map((r) => (
                  <li
                    key={r.triple_id}
                    className="text-sm flex items-baseline gap-2"
                  >
                    <span className="text-tm-muted text-xs w-32 shrink-0">
                      {r.predicate}
                    </span>
                    <button
                      onClick={() => focusEntity(r.target_id)}
                      className="text-tm-text hover:text-tm-accent transition text-left flex-1 truncate"
                    >
                      {r.target_name}
                    </button>
                    <span className="text-xs font-mono text-tm-muted">
                      {fmtNum(r.confidence)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── Graph: backlinks ───────────────────────────────────────── */}
          <Section title="backlinks" count={dump.relations_in.length}>
            {dump.relations_in.length === 0 ? (
              <p className="text-xs text-tm-muted">none</p>
            ) : (
              <ul className="space-y-1">
                {dump.relations_in.map((b) => (
                  <li
                    key={b.triple_id}
                    className="text-sm flex items-baseline gap-2"
                  >
                    <button
                      onClick={() => focusEntity(b.source_id)}
                      className="text-tm-text hover:text-tm-accent transition text-left w-40 truncate shrink-0"
                    >
                      {b.source_name}
                    </button>
                    <span className="text-tm-muted text-xs flex-1">
                      {b.predicate}
                    </span>
                    <span className="text-xs font-mono text-tm-muted">
                      {fmtNum(b.confidence)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── Vector neighbors ───────────────────────────────────────── */}
          <Section
            title="vector neighbors"
            count={dump.vector_neighbors.length}
          >
            {dump.vector_neighbors.length === 0 ? (
              <p className="text-xs text-tm-muted">
                No embedding for this entity (or the vector store is empty).
              </p>
            ) : (
              <ul className="space-y-1">
                {dump.vector_neighbors.map((n) => (
                  <li
                    key={n.entity_id}
                    className="text-sm flex items-baseline gap-2"
                  >
                    <button
                      onClick={() => focusEntity(n.entity_id)}
                      className="text-tm-text hover:text-tm-accent text-left flex-1 truncate"
                    >
                      {n.name}
                    </button>
                    <span className="text-xs text-tm-muted">
                      {n.entity_type}
                    </span>
                    <span className="text-xs font-mono text-tm-accent">
                      {fmtPct(n.similarity)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── K-hop neighbors ───────────────────────────────────────── */}
          <Section
            title="2-hop graph neighborhood"
            count={dump.k_hop_neighbors.length}
            defaultOpen={false}
          >
            {dump.k_hop_neighbors.length === 0 ? (
              <p className="text-xs text-tm-muted">no hops</p>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {dump.k_hop_neighbors.map((h) => (
                  <button
                    key={h.entity_id}
                    onClick={() => focusEntity(h.entity_id)}
                    className="text-xs px-2 py-0.5 rounded border border-tm-border bg-tm-surface/30 text-tm-text hover:border-tm-accent hover:text-tm-accent transition-colors"
                    title={h.entity_type}
                  >
                    {h.name}
                  </button>
                ))}
              </div>
            )}
          </Section>

          {/* ── Belief state ───────────────────────────────────────────── */}
          <Section title="belief state" count={dump.belief_rows.length}>
            {dump.belief_rows.length === 0 ? (
              <p className="text-xs text-tm-muted">
                No triple touches this entity.
              </p>
            ) : (
              <ul className="space-y-1">
                {dump.belief_rows.map((r) => (
                  <li
                    key={r.triple_id}
                    className="text-sm flex items-baseline gap-2"
                  >
                    <StatusPill status={r.status} />
                    <span className="text-tm-text truncate flex-1">
                      {r.subject}{" "}
                      <span className="text-tm-muted">{r.predicate}</span>{" "}
                      {r.object}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── Contradictions ─────────────────────────────────────────── */}
          {dump.contradictions.length > 0 && (
            <Section
              title="contradictions"
              count={dump.contradictions.length}
            >
              <ul className="space-y-1">
                {dump.contradictions.map((c) => (
                  <li
                    key={c.id}
                    className="text-xs flex items-baseline gap-2 font-mono"
                  >
                    <span className="text-rose-400">⚡</span>
                    <span className="text-tm-muted">
                      {shortId(c.triple_a)} ⇄ {shortId(c.triple_b)}
                    </span>
                    <span className="text-tm-text">
                      sim {fmtNum(c.cosine_similarity)}
                    </span>
                    <span className="text-tm-muted">
                      {c.detected_at.slice(0, 10)}
                    </span>
                    {c.resolution && (
                      <span className="text-emerald-300">→ {c.resolution}</span>
                    )}
                  </li>
                ))}
              </ul>
            </Section>
          )}

          {/* ── Provenance (bitemporal history) ────────────────────────── */}
          <Section
            title="provenance"
            count={dump.provenance.length}
            defaultOpen={false}
          >
            {dump.provenance.length === 0 ? (
              <p className="text-xs text-tm-muted">
                Only the current version is recorded.
              </p>
            ) : (
              <ul className="space-y-1">
                {dump.provenance.map((p, i) => (
                  <li
                    key={i}
                    className="text-xs flex items-baseline gap-2 font-mono"
                  >
                    <span className="text-tm-muted w-32 shrink-0">
                      {p.recorded_at.slice(0, 16).replace("T", " ")}
                    </span>
                    <span className="text-tm-text flex-1 truncate">
                      {p.name}{" "}
                      <span className="text-tm-muted">({p.entity_type})</span>
                    </span>
                    <span className="text-tm-muted">conf {fmtNum(p.confidence)}</span>
                    {p.superseded_at && (
                      <span className="text-amber-400">superseded</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── Recent traces touching this entity ─────────────────────── */}
          <Section
            title="recent traces"
            count={dump.recent_traces.length}
            defaultOpen={false}
          >
            {dump.recent_traces.length === 0 ? (
              <p className="text-xs text-tm-muted">
                No trace in the recent buffer references this entity.
              </p>
            ) : (
              <ul className="space-y-1">
                {dump.recent_traces.map((t) => (
                  <li
                    key={t.trace_id}
                    className="text-xs flex items-baseline gap-2"
                  >
                    <span className="text-tm-muted font-mono w-32 shrink-0">
                      {t.created_at.slice(0, 16).replace("T", " ")}
                    </span>
                    <span className="text-tm-accent text-xs w-16 shrink-0">
                      {t.event_type}
                    </span>
                    {t.retrieval_arm !== null && (
                      <span className="text-tm-muted font-mono shrink-0">
                        arm {t.retrieval_arm}
                      </span>
                    )}
                    <span className="text-tm-text truncate flex-1">
                      {t.raw_text ? t.raw_text.slice(0, 120) : "(no text)"}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Section>

          {/* ── Captured-signal neighbors ──────────────────────────────── */}
          <Section
            title="signal neighbors (raw captures)"
            count={dump.signal_neighbors.length}
            defaultOpen={false}
          >
            {dump.signal_neighbors.length === 0 ? (
              <p className="text-xs text-tm-muted">
                No raw signal embedded close to this entity.
              </p>
            ) : (
              <ul className="space-y-1">
                {dump.signal_neighbors.map((s) => (
                  <li key={s.signal_id} className="text-xs flex flex-col gap-0.5">
                    <div className="flex items-baseline gap-2">
                      <span className="text-tm-muted font-mono">
                        {s.source}
                      </span>
                      <span className="text-tm-muted font-mono">
                        {s.created_at.slice(0, 16).replace("T", " ")}
                      </span>
                      <span className="text-tm-accent font-mono ml-auto">
                        {fmtPct(s.similarity)}
                      </span>
                    </div>
                    <p className="text-tm-text leading-snug break-words">
                      {s.raw_text}
                    </p>
                  </li>
                ))}
              </ul>
            )}
          </Section>
        </>
      )}
    </div>
  );
}
