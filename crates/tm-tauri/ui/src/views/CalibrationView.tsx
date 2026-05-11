import { useEffect, useState } from "react";
import { getBrief, getDashboard, type BriefView, type DashboardStats } from "../api";

// UI-11 — calibration panel. Surfaces the numbers that prove the
// world model is doing something: predictions made, outcome ledger
// counts, contradiction count, Brier-style accuracy proxy. The real
// Brier score arrives with the f_outcome calibration work (Q-7
// scheduled Q4); for the seed deck this shows the available signals
// honestly without inventing a number.

interface Calibration {
  totalPredictions: number;
  outcomesResolved: number;
  outcomesBetter: number;
  outcomesWorse: number;
  outcomesAsExpected: number;
  contradictions: number;
  resolvedRate: number;
}

export default function CalibrationView() {
  const [brief, setBrief] = useState<BriefView | null>(null);
  const [dash, setDash] = useState<DashboardStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([getBrief(), getDashboard()])
      .then(([b, d]) => {
        setBrief(b);
        setDash(d);
      })
      .catch((e) => setError(String(e)));
  }, []);

  let calib: Calibration | null = null;
  if (brief) {
    const open = brief.counts.open;
    const overdue = brief.counts.overdue;
    const resolved = brief.counts.resolved;
    const totalPredictions = open + overdue + resolved;
    calib = {
      totalPredictions,
      outcomesResolved: resolved,
      outcomesBetter: 0, // ledger polarity not exposed in BriefCounts yet
      outcomesWorse: 0,
      outcomesAsExpected: 0,
      contradictions: brief.counts.contradictions,
      resolvedRate: totalPredictions > 0 ? resolved / totalPredictions : 0,
    };
  }

  return (
    <div className="max-w-3xl mx-auto space-y-6">
      <header>
        <h2 className="text-2xl font-semibold text-tm-text">Calibration</h2>
        <p className="text-sm text-tm-muted mt-1">
          How well TraceMind's world model is tracking your reality.
        </p>
      </header>

      {error && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200">
          {error}
        </div>
      )}

      {calib && (
        <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
          <Metric
            label="Predictions tracked"
            value={calib.totalPredictions.toString()}
            hint="commitments + outcomes the model has seen"
          />
          <Metric
            label="Resolution rate"
            value={`${(calib.resolvedRate * 100).toFixed(0)}%`}
            hint="of tracked predictions that closed (resolved or retracted)"
            colorOnRange={[
              { gte: 0.6, color: "text-green-400" },
              { gte: 0.3, color: "text-amber-400" },
            ]}
          />
          <Metric
            label="Contradictions"
            value={calib.contradictions.toString()}
            hint="conflicting facts surfaced (lower = healthier world model)"
            colorOnRange={[{ gte: 5, color: "text-amber-400" }]}
          />
          <Metric
            label="Total entities"
            value={(dash?.entity_count ?? 0).toString()}
            hint="distinct people / places / projects in memory"
          />
          <Metric
            label="Total triples"
            value={(dash?.triple_count ?? 0).toString()}
            hint="relationships the graph has learned"
          />
          <Metric
            label="Trace events"
            value={(dash?.trace_count ?? 0).toString()}
            hint="audit-log entries (every ingest + query is logged)"
          />
        </div>
      )}

      <section className="bg-tm-surface border border-tm-border rounded-lg p-4">
        <h3 className="text-sm font-medium text-tm-text mb-2">Bandit arms (LinUCB)</h3>
        <p className="text-xs text-tm-muted mb-3">
          The retrieval engine picks one of five strategies per query. "Avg reward" tracks
          which arms your feedback has voted up.
        </p>
        {dash && (
          <table className="w-full text-xs">
            <thead className="text-tm-muted">
              <tr>
                <th className="text-left font-normal py-1">Arm</th>
                <th className="text-left font-normal py-1">Name</th>
                <th className="text-right font-normal py-1">Pulls</th>
                <th className="text-right font-normal py-1">Avg reward</th>
              </tr>
            </thead>
            <tbody>
              {dash.bandit_arms.map((a) => (
                <tr key={a.arm} className="border-t border-tm-border/40">
                  <td className="py-1.5">{a.arm}</td>
                  <td className="py-1.5 capitalize">{a.name}</td>
                  <td className="py-1.5 text-right">{a.pulls}</td>
                  <td className="py-1.5 text-right">{a.avg_reward.toFixed(3)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <p className="text-xs text-tm-muted">
        Brier score + per-quarter accuracy charts ship with the world-model v1 (Q-7, Q4 2026).
      </p>
    </div>
  );
}

function Metric({
  label,
  value,
  hint,
  colorOnRange,
}: {
  label: string;
  value: string;
  hint: string;
  colorOnRange?: { gte: number; color: string }[];
}) {
  let color = "text-tm-text";
  if (colorOnRange) {
    const numeric = parseFloat(value);
    if (!Number.isNaN(numeric)) {
      for (const r of colorOnRange) {
        if (numeric >= r.gte) {
          color = r.color;
          break;
        }
      }
    }
  }
  return (
    <div className="bg-tm-surface border border-tm-border rounded p-3">
      <p className="text-[10px] uppercase tracking-wider text-tm-muted">{label}</p>
      <p className={`text-2xl font-medium mt-1 ${color}`}>{value}</p>
      <p className="text-[11px] text-tm-muted mt-1">{hint}</p>
    </div>
  );
}
