import { useEffect, useState, useRef } from "react";
import { getDashboard, demoIngest, getRecommendations, entityClick, toggleCapture, getCaptureStatus, getSurprising, getEntityTrends, consolidateMemory, type DashboardStats, type RecommendationInfo, type CaptureEvent, type SurprisingEntity, type TrendPoint } from "../api";
import { listen } from "@tauri-apps/api/event";

function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
      <p className="text-tm-muted text-xs uppercase tracking-wider">{label}</p>
      <p className="text-3xl font-bold mt-1">{value}</p>
      {sub && <p className="text-xs text-tm-muted mt-1">{sub}</p>}
    </div>
  );
}

export default function Dashboard() {
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [error, setError] = useState("");
  const [demoLoading, setDemoLoading] = useState(false);
  const [demoResult, setDemoResult] = useState<string | null>(null);
  const [recommendations, setRecommendations] = useState<RecommendationInfo[]>([]);
  const [captureOn, setCaptureOn] = useState(true);
  const [captureFeed, setCaptureFeed] = useState<CaptureEvent[]>([]);
  const feedRef = useRef<CaptureEvent[]>([]);
  const [surprising, setSurprising] = useState<SurprisingEntity[]>([]);
  const [trends, setTrends] = useState<TrendPoint[]>([]);
  const [consolidating, setConsolidating] = useState(false);
  const [consolidateResult, setConsolidateResult] = useState<string | null>(null);

  const refresh = () => {
    getDashboard()
      .then(setStats)
      .catch((e) => setError(String(e)));
  };

  const handleDemo = async () => {
    setDemoLoading(true);
    setDemoResult(null);
    try {
      const res = await demoIngest();
      setDemoResult(`Ingested ${res.texts_ingested} texts → ${res.total_entities} entities, ${res.total_triples} triples`);
      refresh();
    } catch (e) {
      setDemoResult(`Error: ${String(e)}`);
    } finally {
      setDemoLoading(false);
    }
  };

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 3000);
    return () => clearInterval(interval);
  }, []);

  // Fetch recommendations every 30s
  useEffect(() => {
    const fetchRecs = () => {
      getRecommendations(5).then(setRecommendations).catch(() => {});
    };
    fetchRecs();
    const interval = setInterval(fetchRecs, 30000);
    return () => clearInterval(interval);
  }, []);

  // Load capture status
  useEffect(() => {
    getCaptureStatus().then(setCaptureOn).catch(() => {});
  }, []);

  // Fetch surprising entities every 30s
  useEffect(() => {
    const fetch = () => { getSurprising(5).then(setSurprising).catch(() => {}); };
    fetch();
    const interval = setInterval(fetch, 30000);
    return () => clearInterval(interval);
  }, []);

  // Fetch entity trends every 60s
  useEffect(() => {
    const fetch = () => { getEntityTrends().then(setTrends).catch(() => {}); };
    fetch();
    const interval = setInterval(fetch, 60000);
    return () => clearInterval(interval);
  }, []);

  // Listen for live capture events
  useEffect(() => {
    const unlisten = listen<CaptureEvent>("capture-event", (event) => {
      feedRef.current = [event.payload, ...feedRef.current].slice(0, 20);
      setCaptureFeed([...feedRef.current]);
      // Auto-refresh stats when new data comes in
      refresh();
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  const handleToggleCapture = async () => {
    const next = !captureOn;
    await toggleCapture(next);
    setCaptureOn(next);
  };

  if (error) {
    return <div className="text-tm-red p-4">Error: {error}</div>;
  }

  if (!stats) {
    return <div className="text-tm-muted p-4">Loading...</div>;
  }

  const totalPulls = stats.bandit_arms.reduce((s, a) => s + a.pulls, 0);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Dashboard</h2>
        <div className="flex items-center gap-3">
          {demoResult && (
            <span className="text-xs text-tm-green">{demoResult}</span>
          )}
          <button
            onClick={handleToggleCapture}
            className={`text-xs px-4 py-1.5 rounded font-medium transition-colors ${
              captureOn
                ? "bg-tm-green/20 text-tm-green border border-tm-green/40 hover:bg-tm-green/30"
                : "bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text"
            }`}
          >
            {captureOn ? "Capture ON" : "Capture OFF"}
          </button>
          <button
            onClick={async () => {
              setConsolidating(true);
              setConsolidateResult(null);
              try {
                const r = await consolidateMemory();
                setConsolidateResult(`+${r.entities_strengthened} -${r.entities_decayed} pruned:${r.entities_pruned} merged:${r.entities_merged}`);
                refresh();
              } catch (e) { setConsolidateResult(`Error: ${e}`); }
              finally { setConsolidating(false); }
            }}
            disabled={consolidating}
            className="text-xs px-4 py-1.5 rounded bg-tm-surface border border-tm-accent2/40 text-tm-accent2 hover:bg-tm-accent2/10 transition-colors disabled:opacity-50"
          >
            {consolidating ? "Consolidating..." : "Sleep"}
          </button>
          {consolidateResult && (
            <span className="text-xs text-tm-muted">{consolidateResult}</span>
          )}
          <button
            onClick={handleDemo}
            disabled={demoLoading}
            className="text-xs px-4 py-1.5 rounded bg-tm-accent text-white hover:bg-tm-accent/80 transition-colors disabled:opacity-50"
          >
            {demoLoading ? "Loading..." : "Load Demo Data"}
          </button>
          <button
            onClick={refresh}
            className="text-xs px-3 py-1.5 rounded bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text transition-colors"
          >
            Refresh
          </button>
        </div>
      </div>

      {/* Stat cards */}
      <div className="grid grid-cols-4 gap-4">
        <StatCard label="Entities" value={stats.entity_count} />
        <StatCard label="Triples" value={stats.triple_count} />
        <StatCard label="Traces" value={stats.trace_count} />
        <StatCard label="Queries" value={totalPulls} sub="bandit pulls" />
      </div>

      {/* Bandit arms */}
      <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
          Retrieval Strategy (UCB Bandit)
        </h3>
        <div className="grid grid-cols-4 gap-3">
          {stats.bandit_arms.map((arm) => {
            const pct = totalPulls > 0 ? ((arm.pulls / totalPulls) * 100).toFixed(0) : "0";
            return (
              <div key={arm.arm} className="p-3 rounded bg-tm-bg border border-tm-border">
                <p className="text-sm font-medium">{arm.name}</p>
                <div className="flex items-baseline gap-2 mt-1">
                  <span className="text-lg font-bold">{arm.pulls}</span>
                  <span className="text-xs text-tm-muted">pulls ({pct}%)</span>
                </div>
                <div className="mt-2 h-1.5 bg-tm-border rounded-full overflow-hidden">
                  <div
                    className="h-full bg-tm-accent rounded-full transition-all"
                    style={{ width: `${Math.min(arm.avg_reward * 100, 100)}%` }}
                  />
                </div>
                <p className="text-xs text-tm-muted mt-1">
                  reward: {arm.avg_reward.toFixed(2)}
                </p>
              </div>
            );
          })}
        </div>
      </div>

      {/* Entity Trends sparkline */}
      {trends.length > 1 && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
          <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
            Entity Growth (7 days)
          </h3>
          <div className="flex items-end gap-1 h-16">
            {(() => {
              const max = Math.max(...trends.map(t => t.entity_count), 1);
              return trends.map((t, i) => (
                <div key={i} className="flex-1 flex flex-col items-center gap-1">
                  <div
                    className="w-full bg-tm-accent/60 rounded-t transition-all"
                    style={{ height: `${(t.entity_count / max) * 100}%`, minHeight: t.entity_count > 0 ? 4 : 1 }}
                  />
                  <span className="text-[9px] text-tm-muted">{t.date.slice(5)}</span>
                </div>
              ));
            })()}
          </div>
        </div>
      )}

      {/* Surprising entities today */}
      {surprising.length > 0 && (
        <div className="bg-tm-surface border border-tm-yellow/20 rounded-lg p-5">
          <h3 className="text-sm font-medium text-tm-yellow uppercase tracking-wider mb-3">
            Most Surprising Today
          </h3>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
            {surprising.map((s) => (
              <button
                key={s.entity_id}
                onClick={() => entityClick(s.entity_id)}
                className="p-3 rounded bg-tm-bg border border-tm-border hover:border-tm-yellow/40 transition-colors text-left"
              >
                <div className="flex items-center gap-2 mb-1">
                  <span className="text-sm font-medium">{s.entity_name}</span>
                  <span className="text-xs text-tm-muted">{s.entity_type}</span>
                </div>
                <div className="flex gap-3 text-xs text-tm-muted">
                  <span>novelty: {(s.novelty * 100).toFixed(0)}%</span>
                  <span>recency: {(s.recency * 100).toFixed(0)}%</span>
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Live capture feed */}
      {captureFeed.length > 0 && (
        <div className="bg-tm-surface border border-tm-green/20 rounded-lg p-5">
          <div className="flex items-center gap-2 mb-3">
            <div className="w-2 h-2 rounded-full bg-tm-green animate-pulse" />
            <h3 className="text-sm font-medium text-tm-green uppercase tracking-wider">
              Live Capture Feed
            </h3>
            <span className="text-xs text-tm-muted ml-auto">{captureFeed.length} recent</span>
          </div>
          <div className="space-y-2 max-h-48 overflow-y-auto">
            {captureFeed.map((evt, i) => (
              <div key={i} className="flex items-center gap-3 p-2 rounded bg-tm-bg border border-tm-border">
                <span className={`text-xs font-mono px-1.5 py-0.5 rounded ${
                  evt.source === "clipboard" ? "bg-tm-yellow/20 text-tm-yellow"
                    : evt.source === "chatgpt" ? "bg-emerald-500/20 text-emerald-400"
                    : evt.source === "claude" ? "bg-orange-500/20 text-orange-400"
                    : evt.source === "gemini" ? "bg-blue-400/20 text-blue-300"
                    : evt.source === "editor" ? "bg-indigo-500/20 text-indigo-400"
                    : "bg-tm-accent2/20 text-tm-accent2"
                }`}>
                  {evt.source}
                </span>
                <span className="text-sm flex-1 truncate">{evt.text}</span>
                <span className="text-xs text-tm-muted whitespace-nowrap">
                  {evt.entities_count}e / {evt.triples_count}t
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Recommendations */}
      {recommendations.length > 0 && (
        <div className="bg-tm-surface border border-tm-accent/20 rounded-lg p-5">
          <h3 className="text-sm font-medium text-tm-accent uppercase tracking-wider mb-3">
            Suggested for You
          </h3>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
            {recommendations.map((rec) => (
              <button
                key={rec.entity_id}
                onClick={() => entityClick(rec.entity_id)}
                className="p-3 rounded bg-tm-bg border border-tm-border hover:border-tm-accent/40 transition-colors text-left"
              >
                <div className="flex items-center gap-2 mb-1">
                  <span className="text-sm font-medium">{rec.entity_name}</span>
                  <span className="text-xs text-tm-muted">{rec.entity_type}</span>
                </div>
                <p className="text-xs text-tm-muted">{rec.reason}</p>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Recent activity */}
      <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <h3 className="text-sm font-medium text-tm-muted uppercase tracking-wider mb-3">
          Recent Activity
        </h3>
        {stats.recent_traces.length === 0 ? (
          <p className="text-sm text-tm-muted">No traces yet. Ingest some memories to get started.</p>
        ) : (
          <div className="space-y-2">
            {stats.recent_traces.map((trace) => (
              <div
                key={trace.id}
                className="flex items-center gap-3 p-3 rounded bg-tm-bg border border-tm-border"
              >
                <span
                  className={`text-xs font-mono px-2 py-0.5 rounded ${
                    trace.event_type === "Ingest"
                      ? "bg-tm-accent/20 text-tm-accent"
                      : "bg-tm-accent2/20 text-tm-accent2"
                  }`}
                >
                  {trace.event_type}
                </span>
                <span className="text-sm flex-1 truncate">
                  {trace.raw_text || "(no text)"}
                </span>
                <span className="text-xs text-tm-muted whitespace-nowrap">
                  {trace.entities_count}e / {trace.triples_count}t
                </span>
                {trace.retrieval_latency_ms !== null && (
                  <span className="text-xs text-tm-muted">{trace.retrieval_latency_ms}ms</span>
                )}
                <span className="text-xs text-tm-muted">{trace.created_at}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
