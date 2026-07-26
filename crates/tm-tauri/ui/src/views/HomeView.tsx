import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getBrainSnapshot,
  getTraces,
  ingestText,
  modeCurrent,
  modeEnd,
  modeEnterFocus,
  modeEnterPrivate,
  queryMemory,
  type BrainSnapshot,
  type IngestResponse,
  type ModeStatus,
  type QueryResponse,
  type TraceInfo,
} from "../api";

// HomeView — the new default consumer surface.
//
// One screen answers three questions:
//   1. What mode am I in right now, and can I change it in one click?
//   2. Can I capture a thought without navigating anywhere?
//   3. What did I capture today?
//
// Everything below is fed by real backend calls. No mock data, no
// placeholders. Empty states are honest — they say "you haven't
// captured anything yet", not "here is some sample content".

type Toast = { kind: "ok" | "err"; msg: string };

const TYPE_ACCENT: Record<string, string> = {
  Person: "text-sky-300 bg-sky-500/10 ring-sky-500/20",
  Organization: "text-fuchsia-300 bg-fuchsia-500/10 ring-fuchsia-500/20",
  Technology: "text-emerald-300 bg-emerald-500/10 ring-emerald-500/20",
  Concept: "text-slate-300 bg-slate-500/10 ring-slate-500/20",
  Project: "text-pink-300 bg-pink-500/10 ring-pink-500/20",
  Decision: "text-orange-300 bg-orange-500/10 ring-orange-500/20",
  Event: "text-rose-300 bg-rose-500/10 ring-rose-500/20",
  Location: "text-amber-300 bg-amber-500/10 ring-amber-500/20",
  File: "text-yellow-300 bg-yellow-500/10 ring-yellow-500/20",
  Url: "text-cyan-300 bg-cyan-500/10 ring-cyan-500/20",
};

function chipClass(t: string): string {
  return TYPE_ACCENT[t] ?? "text-tm-muted bg-tm-border/40 ring-tm-border";
}

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

function formatClock(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}

function greeting(): string {
  const h = new Date().getHours();
  if (h < 5) return "Late night";
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

interface HomeViewProps {
  onOpenAsk?: () => void;
  onOpenReview?: () => void;
}

export default function HomeView({ onOpenAsk, onOpenReview }: HomeViewProps) {
  const [mode, setMode] = useState<ModeStatus | null>(null);
  const [focusIntent, setFocusIntent] = useState("");
  const [showFocusForm, setShowFocusForm] = useState(false);
  const [captureText, setCaptureText] = useState("");
  const [capturing, setCapturing] = useState(false);
  const [lastCapture, setLastCapture] = useState<IngestResponse | null>(null);
  const [traces, setTraces] = useState<TraceInfo[]>([]);
  const [tracesLoading, setTracesLoading] = useState(false);
  const [snapshot, setSnapshot] = useState<BrainSnapshot | null>(null);
  const [askQuery, setAskQuery] = useState("");
  const [askResult, setAskResult] = useState<QueryResponse | null>(null);
  const [asking, setAsking] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  const captureRef = useRef<HTMLTextAreaElement>(null);

  const flash = useCallback((t: Toast, ms = 2200) => {
    setToast(t);
    window.setTimeout(() => setToast(null), ms);
  }, []);

  const loadMode = useCallback(async () => {
    try {
      setMode(await modeCurrent());
    } catch (e) {
      /* mode is optional context — silently ignore */
      console.warn("mode load failed", e);
    }
  }, []);

  const loadTraces = useCallback(async () => {
    setTracesLoading(true);
    try {
      const rows = await getTraces(50);
      setTraces(rows);
    } catch (e) {
      console.warn("traces load failed", e);
    } finally {
      setTracesLoading(false);
    }
  }, []);

  const loadSnapshot = useCallback(async () => {
    try {
      setSnapshot(await getBrainSnapshot());
    } catch (e) {
      /* snapshot failures shouldn't block the surface */
      console.warn("snapshot load failed", e);
    }
  }, []);

  useEffect(() => {
    loadMode();
    loadTraces();
    loadSnapshot();
  }, [loadMode, loadTraces, loadSnapshot]);

  // Refresh mode every 30s in case a Focus session times out on the
  // backend — the ModeManager auto-ends at the end of a bounded
  // session and we want the UI to reflect that.
  useEffect(() => {
    const id = window.setInterval(loadMode, 30_000);
    return () => window.clearInterval(id);
  }, [loadMode]);

  const todayTraces = useMemo(() => {
    const key = todayIso();
    return traces.filter(
      (t) => t.event_type === "Ingest" && t.created_at.startsWith(key),
    );
  }, [traces]);

  const olderIngests = useMemo(() => {
    const key = todayIso();
    return traces
      .filter((t) => t.event_type === "Ingest" && !t.created_at.startsWith(key))
      .slice(0, 5);
  }, [traces]);

  // Which ambient sources have fed memory (used for the "active sources" strip).
  const activeSources = useMemo(() => {
    const counts = new Map<string, number>();
    for (const t of traces) {
      if (t.event_type !== "Ingest" || !t.source) continue;
      counts.set(t.source, (counts.get(t.source) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [traces]);

  const captureBlocked = mode?.mode === "private";

  const runCapture = async () => {
    const t = captureText.trim();
    if (!t || capturing) return;
    if (captureBlocked) {
      flash({ kind: "err", msg: "Private mode — capture is paused" });
      return;
    }
    setCapturing(true);
    try {
      const res = await ingestText(t);
      setLastCapture(res);
      setCaptureText("");
      flash({
        kind: "ok",
        msg: `Captured · ${res.entities.length} ${
          res.entities.length === 1 ? "entity" : "entities"
        }`,
      });
      // Nudge derivative surfaces so the timeline & counters update.
      loadTraces();
      loadSnapshot();
    } catch (e) {
      flash({ kind: "err", msg: `Capture failed: ${e}` });
    } finally {
      setCapturing(false);
    }
  };

  const runAsk = async () => {
    const q = askQuery.trim();
    if (!q || asking) return;
    setAsking(true);
    try {
      const res = await queryMemory(q);
      setAskResult(res);
    } catch (e) {
      flash({ kind: "err", msg: `Ask failed: ${e}` });
    } finally {
      setAsking(false);
    }
  };

  const goAmbient = async () => {
    try {
      await modeEnd();
      setShowFocusForm(false);
      await loadMode();
      flash({ kind: "ok", msg: "Ambient — capturing normally" });
    } catch (e) {
      flash({ kind: "err", msg: String(e) });
    }
  };

  const enterFocus = async () => {
    const i = focusIntent.trim();
    if (!i) return;
    try {
      await modeEnterFocus(i, 30 * 60);
      setShowFocusForm(false);
      setFocusIntent("");
      await loadMode();
      flash({ kind: "ok", msg: `Focus: ${i} · 30 min` });
    } catch (e) {
      flash({ kind: "err", msg: String(e) });
    }
  };

  const enterPrivate = async () => {
    try {
      await modeEnterPrivate(15 * 60);
      setShowFocusForm(false);
      await loadMode();
      flash({ kind: "ok", msg: "Private — watchers paused for 15 min" }, 2600);
    } catch (e) {
      flash({ kind: "err", msg: String(e) });
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-8">
      {/* ────────── Hero: greeting + mode + stats ────────── */}
      <header className="flex items-start justify-between gap-6">
        <div>
          <p className="text-xs uppercase tracking-[0.18em] text-tm-muted">
            {new Date().toLocaleDateString(undefined, {
              weekday: "long",
              month: "long",
              day: "numeric",
            })}
          </p>
          <h1 className="text-3xl font-semibold text-tm-text mt-1 tracking-tight">
            {greeting()}.
          </h1>
          <p className="text-sm text-tm-muted mt-1">
            {snapshot && snapshot.captures_today > 0
              ? `${snapshot.captures_today} ${
                  snapshot.captures_today === 1 ? "capture" : "captures"
                } today · ${snapshot.entity_count} entities in your memory`
              : "Your memory, kept local. It captures itself as you work."}
          </p>
        </div>
        <div className="text-right shrink-0">
          <ModePill mode={mode} />
        </div>
      </header>

      {/* ────────── Active ambient sources ────────── */}
      {activeSources.length > 0 && (
        <section className="rounded-2xl bg-tm-surface/40 border border-tm-border px-4 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[11px] uppercase tracking-wider text-tm-muted mr-1">
              Capturing from
            </span>
            {activeSources.map(([src, n]) => (
              <span key={src} className="inline-flex items-center gap-1.5">
                <SourceBadge source={src} />
                <span className="text-[10px] text-tm-muted tabular-nums">{n}</span>
              </span>
            ))}
            <span className="ml-auto flex items-center gap-1.5 text-[11px] text-tm-green">
              <span className="w-1.5 h-1.5 rounded-full bg-tm-green animate-pulse" />
              live
            </span>
          </div>
        </section>
      )}

      {/* ────────── Mode selector ────────── */}
      <section className="rounded-2xl bg-tm-surface/70 border border-tm-border p-4">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[11px] uppercase tracking-wider text-tm-muted mr-1">
            Mode
          </span>
          <ModeButton
            label="Ambient"
            active={mode?.mode === "ambient" || !mode}
            onClick={goAmbient}
            tone="accent"
            hint="Capture as normal"
          />
          <ModeButton
            label={
              mode?.mode === "focus" && mode.session
                ? `Focus · ${mode.session.name}`
                : "Focus"
            }
            active={mode?.mode === "focus"}
            onClick={() => setShowFocusForm((v) => !v)}
            tone="accent"
            hint="Tag captures with an intent"
          />
          <ModeButton
            label="Private"
            active={mode?.mode === "private"}
            onClick={enterPrivate}
            tone="danger"
            hint="Pause every watcher"
          />
          {mode?.session && !mode.session.ended_at && (
            <span className="ml-auto text-[11px] text-tm-muted">
              since {formatClock(mode.session.started_at)} ·{" "}
              {Math.round(mode.session.duration_secs / 60)}m
            </span>
          )}
        </div>
        {showFocusForm && (
          <div className="flex gap-2 mt-3">
            <input
              type="text"
              autoFocus
              placeholder="What are you focused on?"
              value={focusIntent}
              onChange={(e) => setFocusIntent(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") enterFocus();
                if (e.key === "Escape") setShowFocusForm(false);
              }}
              className="flex-1 bg-tm-bg border border-tm-border rounded-lg px-3 py-2 text-sm placeholder-tm-muted focus:outline-none focus:border-tm-accent"
            />
            <button
              onClick={enterFocus}
              disabled={!focusIntent.trim()}
              className="px-4 py-2 rounded-lg bg-tm-accent text-white text-sm font-medium disabled:opacity-40"
            >
              Start
            </button>
          </div>
        )}
      </section>

      {/* ────────── Quick capture ────────── */}
      <section className="space-y-3">
        <label
          htmlFor="quick-capture"
          className="flex items-baseline justify-between"
        >
          <span className="text-sm font-medium text-tm-text">
            Capture a thought
          </span>
          <span className="text-[11px] text-tm-muted">⌘ + Return</span>
        </label>
        <div
          className={`relative rounded-2xl border transition-colors ${
            captureBlocked
              ? "border-tm-red/30 bg-tm-red/[0.03]"
              : "border-tm-border bg-tm-surface hover:border-tm-border/80 focus-within:border-tm-accent/60"
          }`}
        >
          <textarea
            id="quick-capture"
            ref={captureRef}
            value={captureText}
            onChange={(e) => setCaptureText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) runCapture();
            }}
            disabled={captureBlocked}
            placeholder={
              captureBlocked
                ? "Private mode is on — nothing will be captured."
                : "Met Jane at Blue Bottle to talk pricing…"
            }
            rows={3}
            className="w-full resize-none bg-transparent px-4 py-3 text-sm text-tm-text placeholder-tm-muted focus:outline-none disabled:cursor-not-allowed"
          />
          <div className="flex items-center justify-between px-4 py-2 border-t border-tm-border/60">
            <span className="text-[11px] text-tm-muted">
              {captureText.trim().length > 0
                ? `${captureText.trim().length} chars`
                : "Local only. Nothing leaves your machine."}
            </span>
            <button
              onClick={runCapture}
              disabled={capturing || !captureText.trim() || captureBlocked}
              className="px-4 py-1.5 rounded-lg bg-tm-accent text-white text-xs font-medium hover:bg-tm-accent/85 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {capturing ? "Capturing…" : "Capture"}
            </button>
          </div>
        </div>

        {lastCapture && (
          <div className="rounded-xl bg-tm-accent/[0.06] border border-tm-accent/20 p-4 space-y-2 animate-[fadeIn_0.24s_ease-out]">
            <div className="flex items-center gap-2 text-xs">
              <span className="w-1.5 h-1.5 rounded-full bg-tm-green" />
              <span className="text-tm-text font-medium">
                Captured · trace {lastCapture.trace_id.slice(0, 8)}
              </span>
              <span className="ml-auto text-tm-muted">
                {lastCapture.entities.length} entities ·{" "}
                {lastCapture.typed_triples.length} relations
              </span>
            </div>
            {lastCapture.entities.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {lastCapture.entities.map((e) => (
                  <span
                    key={e.id}
                    className={`text-[11px] px-2 py-0.5 rounded-full ring-1 ${chipClass(
                      e.entity_type,
                    )}`}
                  >
                    <span className="opacity-70 mr-1">{e.entity_type}</span>
                    {e.name}
                  </span>
                ))}
              </div>
            )}
          </div>
        )}
      </section>

      {/* ────────── Ask ────────── */}
      <section className="space-y-3">
        <div className="flex items-baseline justify-between">
          <span className="text-sm font-medium text-tm-text">Ask your memory</span>
          {onOpenAsk && (
            <button
              onClick={onOpenAsk}
              className="text-[11px] text-tm-muted hover:text-tm-accent transition-colors"
            >
              Open full search →
            </button>
          )}
        </div>
        <div className="rounded-2xl border border-tm-border bg-tm-surface">
          <div className="flex items-center gap-2 px-4 py-2">
            <svg
              className="w-4 h-4 text-tm-muted"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={2}
                d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
              />
            </svg>
            <input
              type="text"
              value={askQuery}
              onChange={(e) => setAskQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") runAsk();
              }}
              placeholder="Who did I meet at Blue Bottle? What did I decide about pricing?"
              className="flex-1 bg-transparent text-sm placeholder-tm-muted focus:outline-none py-1.5"
            />
            <button
              onClick={runAsk}
              disabled={asking || !askQuery.trim()}
              className="text-xs text-tm-accent disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {asking ? "…" : "Ask"}
            </button>
          </div>
          {askResult && (
            <div className="border-t border-tm-border/60 px-4 py-3 space-y-3">
              <p className="text-sm text-tm-text leading-relaxed whitespace-pre-line">
                {askResult.explanation || "No direct answer — try rephrasing."}
              </p>
              {askResult.entities.length > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  {askResult.entities.slice(0, 10).map((e) => (
                    <span
                      key={e.id}
                      className={`text-[11px] px-2 py-0.5 rounded-full ring-1 ${chipClass(
                        e.entity_type,
                      )}`}
                    >
                      {e.name}
                    </span>
                  ))}
                </div>
              )}
              <div className="text-[10px] text-tm-muted font-mono">
                arm {askResult.arm_name} · {askResult.latency_ms}ms
              </div>
            </div>
          )}
        </div>
      </section>

      {/* ────────── Today ────────── */}
      <section className="space-y-3">
        <div className="flex items-baseline justify-between">
          <span className="text-sm font-medium text-tm-text">
            Today's memory
          </span>
          {onOpenReview && (
            <button
              onClick={onOpenReview}
              className="text-[11px] text-tm-muted hover:text-tm-accent transition-colors"
            >
              Review all captures →
            </button>
          )}
        </div>
        {tracesLoading && traces.length === 0 && (
          <div className="rounded-xl border border-tm-border bg-tm-surface/60 p-6 text-sm text-tm-muted">
            Loading…
          </div>
        )}
        {!tracesLoading && todayTraces.length === 0 && (
          <div className="rounded-xl border border-dashed border-tm-border bg-tm-surface/40 p-8 text-center space-y-2">
            <p className="text-sm text-tm-text">Nothing captured today yet.</p>
            <p className="text-xs text-tm-muted">
              Type a thought above, or leave capture running — it will show up
              here.
            </p>
          </div>
        )}
        {todayTraces.length > 0 && (
          <ul className="space-y-2">
            {todayTraces.map((t) => (
              <TraceRow key={t.id} trace={t} />
            ))}
          </ul>
        )}
        {olderIngests.length > 0 && (
          <details className="rounded-xl border border-tm-border bg-tm-surface/40 overflow-hidden">
            <summary className="cursor-pointer px-4 py-2 text-[11px] uppercase tracking-wider text-tm-muted hover:text-tm-text">
              Earlier · {olderIngests.length} recent
            </summary>
            <ul className="border-t border-tm-border/50 divide-y divide-tm-border/30">
              {olderIngests.map((t) => (
                <TraceRow key={t.id} trace={t} compact />
              ))}
            </ul>
          </details>
        )}
      </section>

      {/* ────────── Footer stats — only when we have real data ────────── */}
      {snapshot && (snapshot.entity_count > 0 || snapshot.total_traces > 0) && (
        <footer className="grid grid-cols-4 gap-3 pt-2">
          <StatCell label="entities" value={snapshot.entity_count} />
          <StatCell label="relations" value={snapshot.triple_count} />
          <StatCell label="traces" value={snapshot.total_traces} />
          <StatCell
            label="commitments"
            value={snapshot.open_commitments}
            hint={
              snapshot.overdue_commitments > 0
                ? `${snapshot.overdue_commitments} overdue`
                : undefined
            }
            danger={snapshot.overdue_commitments > 0}
          />
        </footer>
      )}

      {/* Toast */}
      {toast && (
        <div
          className={`fixed bottom-6 left-1/2 -translate-x-1/2 px-4 py-2 rounded-full text-xs shadow-lg z-50 backdrop-blur border ${
            toast.kind === "ok"
              ? "bg-tm-green/15 text-tm-green border-tm-green/30"
              : "bg-tm-red/15 text-tm-red border-tm-red/30"
          }`}
        >
          {toast.msg}
        </div>
      )}

      <style>{`
        @keyframes fadeIn {
          from { opacity: 0; transform: translateY(4px); }
          to { opacity: 1; transform: none; }
        }
      `}</style>
    </div>
  );
}

// ─── Small pieces ───────────────────────────────────────────────────────

function ModePill({ mode }: { mode: ModeStatus | null }) {
  const m = mode?.mode ?? "ambient";
  const label = m === "focus" && mode?.session ? mode.session.name : m;
  const dot =
    m === "private"
      ? "bg-tm-red"
      : m === "focus"
      ? "bg-tm-yellow"
      : "bg-tm-green";
  const ring =
    m === "private"
      ? "ring-tm-red/30"
      : m === "focus"
      ? "ring-tm-yellow/30"
      : "ring-tm-green/30";
  return (
    <div
      className={`inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-tm-surface/80 ring-1 ${ring}`}
    >
      <span
        className={`w-2 h-2 rounded-full ${dot} ${
          m !== "private" ? "animate-pulse" : ""
        }`}
      />
      <span className="text-xs text-tm-text capitalize">{label}</span>
    </div>
  );
}

function ModeButton({
  label,
  active,
  onClick,
  tone,
  hint,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  tone: "accent" | "danger";
  hint: string;
}) {
  const activeCls =
    tone === "danger"
      ? "border-tm-red/60 bg-tm-red/10 text-tm-red"
      : "border-tm-accent/60 bg-tm-accent/10 text-tm-accent";
  const idleCls =
    "border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/[0.03]";
  return (
    <button
      onClick={onClick}
      title={hint}
      className={`px-3 py-1.5 rounded-full text-xs border transition-colors ${
        active ? activeCls : idleCls
      }`}
    >
      {label}
    </button>
  );
}

function StatCell({
  label,
  value,
  hint,
  danger,
}: {
  label: string;
  value: number;
  hint?: string;
  danger?: boolean;
}) {
  return (
    <div className="rounded-xl border border-tm-border bg-tm-surface/40 px-4 py-3">
      <div
        className={`text-xl font-semibold tracking-tight ${
          danger ? "text-tm-red" : "text-tm-text"
        }`}
      >
        {value.toLocaleString()}
      </div>
      <div className="text-[10px] uppercase tracking-wider text-tm-muted mt-0.5">
        {label}
      </div>
      {hint && (
        <div className="text-[10px] text-tm-red mt-0.5">{hint}</div>
      )}
    </div>
  );
}

const SOURCE_META: Record<string, { label: string; icon: string; className: string }> = {
  notes: { label: "Notes", icon: "📝", className: "bg-yellow-500/15 text-yellow-300 border-yellow-500/30" },
  safari: { label: "Safari", icon: "🧭", className: "bg-blue-500/15 text-blue-300 border-blue-500/30" },
  calendar: { label: "Calendar", icon: "📅", className: "bg-red-500/15 text-red-300 border-red-500/30" },
  email: { label: "Mail", icon: "✉️", className: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30" },
  imessage: { label: "Messages", icon: "💬", className: "bg-green-500/15 text-green-300 border-green-500/30" },
  clipboard: { label: "Clipboard", icon: "📋", className: "bg-purple-500/15 text-purple-300 border-purple-500/30" },
};

function SourceBadge({ source }: { source: string }) {
  const meta = SOURCE_META[source] ?? {
    label: source,
    icon: "•",
    className: "bg-tm-border/40 text-tm-muted border-tm-border",
  };
  return (
    <span
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md border text-[10px] font-medium ${meta.className}`}
    >
      <span>{meta.icon}</span>
      {meta.label}
    </span>
  );
}

function TraceRow({ trace, compact }: { trace: TraceInfo; compact?: boolean }) {
  return (
    <li
      className={`bg-tm-surface/70 border border-tm-border rounded-xl ${
        compact ? "px-4 py-2" : "px-4 py-3"
      } flex items-start gap-3 hover:bg-tm-surface transition-colors`}
    >
      <span className="mt-1 text-[10px] font-mono text-tm-muted tabular-nums">
        {formatClock(trace.created_at)}
      </span>
      <div className="flex-1 min-w-0">
        <p
          className={`text-tm-text ${compact ? "text-xs" : "text-sm"} truncate`}
        >
          {trace.raw_text || <em className="text-tm-muted">no snippet</em>}
        </p>
        <div className="flex items-center gap-2 mt-1">
          {trace.source && <SourceBadge source={trace.source} />}
          <span className="text-[10px] text-tm-muted">
            {trace.entities_count} entities · {trace.triples_count} relations
          </span>
        </div>
      </div>
    </li>
  );
}
