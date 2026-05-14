import { useEffect, useState } from "react";
import { BrainSnapshot, getBrainSnapshot } from "../../api";

// Panel 1 — Brain snapshot.
// Single-page health dump that calls cmd_brain_snapshot and renders every
// layer's headline metric in one scan. No interactivity beyond refresh.

function fmtNum(n: number): string {
  return Number.isFinite(n) ? n.toLocaleString() : "—";
}

function fmtAlpha(a: number): string {
  if (!Number.isFinite(a)) return "—";
  return a.toFixed(4);
}

function fmtPct(num: number, den: number): string {
  if (den <= 0) return "—";
  return `${((num / den) * 100).toFixed(1)}%`;
}

function fmtTimestamp(t: string): string {
  const d = new Date(t);
  if (Number.isNaN(d.getTime())) return t;
  return d.toLocaleString();
}

export default function BrainSnapshotPanel() {
  const [snap, setSnap] = useState<BrainSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      setSnap(await getBrainSnapshot());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    load();
  }, []);

  if (loading && !snap) {
    return <p className="text-sm text-tm-muted">Sampling every layer…</p>;
  }
  if (error) {
    return (
      <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200">
        {error}
      </div>
    );
  }
  if (!snap) return null;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <p className="text-xs text-tm-muted">
          Sampled <span className="text-tm-text">{fmtTimestamp(snap.generated_at)}</span> ·{" "}
          <code className="text-tm-accent">{snap.data_dir}</code>
        </p>
        <button
          onClick={load}
          className="text-xs px-2.5 py-1 border border-tm-border rounded text-tm-muted hover:text-tm-text"
        >
          ↻ resample
        </button>
      </div>

      {/* Graph layer */}
      <Section title="Graph" subtitle="entities, triples, communities, contradictions">
        <Grid>
          <Metric label="Entities" value={fmtNum(snap.entity_count)} />
          <Metric label="Triples" value={fmtNum(snap.triple_count)} />
          <Metric label="Communities (Louvain)" value={fmtNum(snap.community_count)} />
          <Metric label="Contradictions" value={fmtNum(snap.contradiction_count)} tone={snap.contradiction_count > 0 ? "warn" : "ok"} />
          <Metric label="Pending relations" value={fmtNum(snap.pending_relation_count)} />
        </Grid>
      </Section>

      {/* Vector layer */}
      <Section title="Vector" subtitle="BGE-small embeddings (cosine)">
        <Grid>
          <Metric label="Dimension" value={fmtNum(snap.vector_dim)} />
          <Metric label="Entities with vector" value={fmtNum(snap.entities_with_vector)} />
          <Metric label="Coverage" value={fmtPct(snap.entities_with_vector, snap.entity_count)} />
        </Grid>
      </Section>

      {/* Bandit layer */}
      <Section
        title="Bandit (controller)"
        subtitle={`5 arms — UCB1 + LinUCB (α=${fmtAlpha(snap.linucb_alpha)})`}
      >
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-tm-muted border-b border-tm-border">
                <th className="py-2 pr-3">Arm</th>
                <th className="py-2 pr-3">Name</th>
                <th className="py-2 pr-3">UCB pulls</th>
                <th className="py-2 pr-3">UCB avg reward</th>
                <th className="py-2 pr-3">LinUCB pulls</th>
                <th className="py-2 pr-3">LinUCB w-mag</th>
              </tr>
            </thead>
            <tbody>
              {snap.bandit_arms.map((r) => (
                <tr key={r.arm} className="border-b border-tm-border/40">
                  <td className="py-1.5 pr-3 text-tm-muted">{r.arm}</td>
                  <td className="py-1.5 pr-3 font-medium text-tm-text">{r.name}</td>
                  <td className="py-1.5 pr-3 tabular-nums">{fmtNum(r.ucb_pulls)}</td>
                  <td className="py-1.5 pr-3 tabular-nums">{r.ucb_avg_reward.toFixed(3)}</td>
                  <td className="py-1.5 pr-3 tabular-nums">{fmtNum(r.linucb_pulls)}</td>
                  <td className="py-1.5 pr-3 tabular-nums">{r.linucb_weight_mag.toFixed(4)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Section>

      {/* Episodic layer */}
      <Section title="Episodic" subtitle="immutable trace log + recent ring buffer">
        <Grid>
          <Metric label="Total traces" value={fmtNum(snap.total_traces)} />
          <Metric
            label="Recent ring"
            value={`${fmtNum(snap.recent_buffer_size)} / ${fmtNum(snap.recent_buffer_capacity)}`}
          />
        </Grid>
        {snap.by_event.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-2">
            {snap.by_event.map((b) => (
              <span
                key={b.event_type}
                className="text-xs px-2 py-1 bg-white/5 border border-tm-border rounded"
              >
                <span className="text-tm-muted">{b.event_type}</span>{" "}
                <span className="text-tm-text">{fmtNum(b.count)}</span>
              </span>
            ))}
          </div>
        )}
      </Section>

      {/* Governance */}
      <Section title="Governance" subtitle="PII regex + confidence gate (today)">
        <Grid>
          <Metric label="Captures today" value={fmtNum(snap.captures_today)} />
          <Metric
            label="Blocks today"
            value={fmtNum(snap.governance_blocks_today)}
            tone={snap.governance_blocks_today > 0 ? "warn" : "ok"}
          />
        </Grid>
      </Section>

      {/* Intent layer */}
      <Section title="Intent" subtitle="commitments, candidates, pattern silences">
        <Grid>
          <Metric label="Open commitments" value={fmtNum(snap.open_commitments)} />
          <Metric
            label="Overdue"
            value={fmtNum(snap.overdue_commitments)}
            tone={snap.overdue_commitments > 0 ? "warn" : "ok"}
          />
          <Metric label="Pending candidates" value={fmtNum(snap.pending_candidates)} />
          <Metric label="Active pattern silences" value={fmtNum(snap.pattern_silences_active)} />
        </Grid>
      </Section>
    </div>
  );
}

function Section({ title, subtitle, children }: { title: string; subtitle: string; children: React.ReactNode }) {
  return (
    <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
      <div className="mb-3">
        <h3 className="text-base font-medium text-tm-text">{title}</h3>
        <p className="text-xs text-tm-muted">{subtitle}</p>
      </div>
      {children}
    </section>
  );
}

function Grid({ children }: { children: React.ReactNode }) {
  return <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-2">{children}</div>;
}

function Metric({ label, value, tone = "neutral" }: { label: string; value: string; tone?: "neutral" | "ok" | "warn" }) {
  const toneClass =
    tone === "warn"
      ? "border-amber-700/60 bg-amber-900/10"
      : tone === "ok"
      ? "border-tm-border/60"
      : "border-tm-border/60";
  return (
    <div className={`bg-black/20 border rounded p-2.5 ${toneClass}`}>
      <p className="text-[10px] uppercase tracking-wider text-tm-muted">{label}</p>
      <p className="text-base font-medium text-tm-text mt-0.5 tabular-nums">{value}</p>
    </div>
  );
}
