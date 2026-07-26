import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ingestionDiscard,
  ingestionRefine,
  ingestionReviewRecent,
  modeCurrent,
  modeEnd,
  modeEnterFocus,
  modeEnterPrivate,
  type ModeStatus,
  type ReceiptRow,
} from "../api";

// I5/I7 — Ingestion Review view.
//
// Renders the daily digest of capture receipts described in
// docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md §3.3. The three actions
// mirror the plan: Keep (no-op ack), Refine (edit + re-ingest through the
// full pipeline), Discard (append to the discard log so the row drops out
// of the default view but the original receipt is preserved for audit —
// per commandment S1).
//
// Grouping is *chronological by day* because the review is a daily
// ritual, not a project-scoped tool. The plan calls this out: "show the
// user their day, not an admin table".

type Grouped = { dayKey: string; label: string; rows: ReceiptRow[] };

function formatDayKey(iso: string): { key: string; label: string } {
  const d = new Date(iso);
  const key = d.toISOString().slice(0, 10);
  const today = new Date().toISOString().slice(0, 10);
  const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);
  let label = key;
  if (key === today) label = "Today";
  else if (key === yesterday) label = "Yesterday";
  else
    label = d.toLocaleDateString(undefined, {
      weekday: "long",
      month: "short",
      day: "numeric",
    });
  return { key, label };
}

function groupByDay(rows: ReceiptRow[]): Grouped[] {
  const buckets: Record<string, Grouped> = {};
  for (const r of rows) {
    const { key, label } = formatDayKey(r.at);
    if (!buckets[key]) buckets[key] = { dayKey: key, label, rows: [] };
    buckets[key].rows.push(r);
  }
  return Object.values(buckets).sort((a, b) => b.dayKey.localeCompare(a.dayKey));
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

export default function IngestionReviewView() {
  const [rows, setRows] = useState<ReceiptRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [showDiscarded, setShowDiscarded] = useState(false);
  const [refiningId, setRefiningId] = useState<string | null>(null);
  const [refineText, setRefineText] = useState("");
  const [flash, setFlash] = useState<string | null>(null);
  const [mode, setMode] = useState<ModeStatus | null>(null);
  const [focusIntent, setFocusIntent] = useState("");
  const [showFocusForm, setShowFocusForm] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const list = await ingestionReviewRecent(200, showDiscarded);
      setRows(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [showDiscarded]);

  const loadMode = useCallback(async () => {
    try {
      setMode(await modeCurrent());
    } catch (e) {
      // Non-fatal — mode selector just renders as "unknown".
      console.warn("mode load failed", e);
    }
  }, []);

  useEffect(() => {
    load();
    loadMode();
  }, [load, loadMode]);

  const goAmbient = async () => {
    try {
      await modeEnd();
      setShowFocusForm(false);
      await loadMode();
      setFlash("Now Ambient");
      window.setTimeout(() => setFlash(null), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  const enterFocus = async () => {
    if (!focusIntent.trim()) return;
    try {
      await modeEnterFocus(focusIntent.trim(), 30 * 60);
      setShowFocusForm(false);
      setFocusIntent("");
      await loadMode();
      setFlash("Focus mode: " + focusIntent.trim());
      window.setTimeout(() => setFlash(null), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  const enterPrivate = async () => {
    try {
      await modeEnterPrivate(15 * 60);
      setShowFocusForm(false);
      await loadMode();
      setFlash("Private mode: watchers paused for 15 min");
      window.setTimeout(() => setFlash(null), 2000);
    } catch (e) {
      setError(String(e));
    }
  };

  const groups = useMemo(() => groupByDay(rows), [rows]);
  const totalToday = useMemo(() => {
    const today = new Date().toISOString().slice(0, 10);
    return rows.filter((r) => r.at.startsWith(today) && !r.discarded).length;
  }, [rows]);

  const handleKeep = (id: string) => {
    setFlash(`Kept ${id.slice(0, 8)}`);
    window.setTimeout(() => setFlash(null), 1500);
  };

  const handleDiscard = async (id: string) => {
    try {
      await ingestionDiscard(id, "user marked in review");
      await load();
      setFlash(`Discarded ${id.slice(0, 8)}`);
      window.setTimeout(() => setFlash(null), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  const startRefine = (row: ReceiptRow) => {
    setRefiningId(row.capture_id);
    setRefineText(row.why_captured);
  };

  const cancelRefine = () => {
    setRefiningId(null);
    setRefineText("");
  };

  const submitRefine = async () => {
    if (!refiningId) return;
    try {
      const res = await ingestionRefine(refiningId, refineText);
      setFlash(`Refined → ${res.entities_extracted} entities`);
      window.setTimeout(() => setFlash(null), 2000);
      cancelRefine();
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="space-y-6">
      <header className="flex items-baseline justify-between">
        <div>
          <h2 className="text-xl font-semibold">Ingestion Review</h2>
          <p className="text-xs text-tm-muted mt-1">
            Librarian, not wire-tap. Every capture is receipted — you decide what stays.
          </p>
        </div>
        <div className="text-right">
          <div className="text-2xl font-semibold text-tm-accent">{totalToday}</div>
          <div className="text-xs text-tm-muted uppercase tracking-wider">today</div>
        </div>
      </header>

      {/* I6/I9 — three-mode selector */}
      <div className="bg-tm-surface border border-tm-border rounded-lg p-4 space-y-3">
        <div className="flex items-center gap-3">
          <span className="text-xs uppercase tracking-wider text-tm-muted">Mode</span>
          <div className="flex gap-1">
            <button
              onClick={goAmbient}
              className={`px-3 py-1.5 rounded text-xs border ${
                mode?.mode === "ambient"
                  ? "border-tm-accent bg-tm-accent/10 text-tm-accent"
                  : "border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/5"
              }`}
            >
              Ambient
            </button>
            <button
              onClick={() => setShowFocusForm((v) => !v)}
              className={`px-3 py-1.5 rounded text-xs border ${
                mode?.mode === "focus"
                  ? "border-tm-accent bg-tm-accent/10 text-tm-accent"
                  : "border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/5"
              }`}
            >
              Focus{mode?.mode === "focus" && mode.session ? `: ${mode.session.name}` : ""}
            </button>
            <button
              onClick={enterPrivate}
              className={`px-3 py-1.5 rounded text-xs border ${
                mode?.mode === "private"
                  ? "border-tm-red bg-tm-red/10 text-tm-red"
                  : "border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/5"
              }`}
            >
              Private
            </button>
          </div>
          {mode?.session && !mode.session.ended_at && (
            <span className="ml-auto text-[11px] text-tm-muted">
              since {new Date(mode.session.started_at).toLocaleTimeString()} ·{" "}
              {Math.round(mode.session.duration_secs / 60)}m
            </span>
          )}
        </div>
        {showFocusForm && (
          <div className="flex gap-2">
            <input
              type="text"
              autoFocus
              placeholder="What are you focused on? (e.g. call-prep-tuesday)"
              value={focusIntent}
              onChange={(e) => setFocusIntent(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") enterFocus();
                if (e.key === "Escape") setShowFocusForm(false);
              }}
              className="flex-1 bg-tm-bg border border-tm-border rounded px-3 py-1.5 text-xs"
            />
            <button
              onClick={enterFocus}
              disabled={!focusIntent.trim()}
              className="px-3 py-1.5 rounded bg-tm-accent text-white text-xs disabled:opacity-50"
            >
              Start Focus
            </button>
          </div>
        )}
      </div>

      <div className="flex items-center gap-3 text-xs">
        <button
          onClick={() => setShowDiscarded((v) => !v)}
          className="px-3 py-1.5 rounded border border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/5"
        >
          {showDiscarded ? "Hide discarded" : "Show discarded"}
        </button>
        <button
          onClick={load}
          className="px-3 py-1.5 rounded border border-tm-border text-tm-muted hover:text-tm-text hover:bg-white/5"
        >
          Reload
        </button>
        {flash && <span className="ml-auto text-tm-green">{flash}</span>}
      </div>

      {error && <div className="text-tm-red text-sm">{error}</div>}
      {loading && <div className="text-tm-muted text-sm">Loading…</div>}

      {!loading && groups.length === 0 && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-8 text-center">
          <p className="text-tm-muted text-sm">
            No captures yet. When capture starts flowing, you will review them here.
          </p>
        </div>
      )}

      {groups.map((g) => (
        <section key={g.dayKey} className="space-y-2">
          <h3 className="text-xs uppercase tracking-wider text-tm-muted">{g.label}</h3>
          <ul className="space-y-2">
            {g.rows.map((r) => (
              <li
                key={r.capture_id}
                className={`bg-tm-surface border border-tm-border rounded-lg p-4 ${
                  r.discarded ? "opacity-60" : ""
                }`}
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 text-xs">
                      <span className="text-tm-muted font-mono">{formatTime(r.at)}</span>
                      <span className="px-1.5 py-0.5 rounded bg-tm-bg border border-tm-border text-tm-muted">
                        {r.source}
                      </span>
                      <span className="text-tm-muted">·</span>
                      <span className="text-tm-muted">{r.modality}</span>
                      {r.app_context && (
                        <>
                          <span className="text-tm-muted">·</span>
                          <span className="text-tm-muted">{r.app_context}</span>
                        </>
                      )}
                      {r.user_intent && (
                        <span className="ml-auto px-2 py-0.5 rounded bg-tm-accent/10 text-tm-accent">
                          {r.user_intent}
                        </span>
                      )}
                      {r.discarded && (
                        <span className="ml-auto px-2 py-0.5 rounded bg-tm-red/10 text-tm-red text-[10px] uppercase">
                          discarded
                        </span>
                      )}
                    </div>
                    <p className="text-sm text-tm-text mt-2">{r.why_captured}</p>
                    <p className="text-[10px] text-tm-muted mt-1 font-mono">
                      {r.capture_id.slice(0, 8)} · {r.size_bytes} bytes
                    </p>
                  </div>

                  {!r.discarded && refiningId !== r.capture_id && (
                    <div className="flex flex-col gap-1 text-xs">
                      <button
                        onClick={() => handleKeep(r.capture_id)}
                        className="px-3 py-1 rounded border border-tm-border hover:bg-white/5"
                      >
                        Keep
                      </button>
                      <button
                        onClick={() => startRefine(r)}
                        className="px-3 py-1 rounded border border-tm-accent text-tm-accent hover:bg-tm-accent/10"
                      >
                        Refine
                      </button>
                      <button
                        onClick={() => handleDiscard(r.capture_id)}
                        className="px-3 py-1 rounded border border-tm-red text-tm-red hover:bg-tm-red/10"
                      >
                        Discard
                      </button>
                    </div>
                  )}
                </div>

                {refiningId === r.capture_id && (
                  <div className="mt-3 space-y-2">
                    <textarea
                      className="w-full bg-tm-bg border border-tm-border rounded-lg px-3 py-2 text-sm h-24"
                      value={refineText}
                      onChange={(e) => setRefineText(e.target.value)}
                      placeholder="Refine the captured text — this will be re-ingested through the full pipeline."
                    />
                    <div className="flex gap-2">
                      <button
                        onClick={submitRefine}
                        disabled={!refineText.trim()}
                        className="px-3 py-1 rounded bg-tm-accent text-white text-xs disabled:opacity-50"
                      >
                        Re-ingest
                      </button>
                      <button
                        onClick={cancelRefine}
                        className="px-3 py-1 rounded border border-tm-border text-xs text-tm-muted"
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                )}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
