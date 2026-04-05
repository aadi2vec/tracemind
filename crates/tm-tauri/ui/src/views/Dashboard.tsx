import { useEffect, useState } from "react";
import { getDashboard, type DashboardStats } from "../api";

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

  const refresh = () => {
    getDashboard()
      .then(setStats)
      .catch((e) => setError(String(e)));
  };

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 3000);
    return () => clearInterval(interval);
  }, []);

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
        <button
          onClick={refresh}
          className="text-xs px-3 py-1.5 rounded bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text transition-colors"
        >
          Refresh
        </button>
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
