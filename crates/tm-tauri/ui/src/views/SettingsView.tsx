import { useEffect, useState } from "react";
import {
  CapturePermissionRow,
  ForgetSourceResult,
  UsageStats,
  forgetCaptureSource,
  getUsageSharePayload,
  getUsageStats,
  listCapturePermissions,
  setCapturePermission,
} from "../api";

// UI-12 + UI-13: settings + privacy panel. Per-source capture
// toggles, audit counters, "forget all from this source," + DP-3
// usage stats + opt-in share payload.

function fmtTime(t: string | null): string {
  if (!t) return "—";
  const d = new Date(t);
  if (Number.isNaN(d.getTime())) return t;
  return d.toLocaleString();
}

function relTime(t: string | null): string {
  if (!t) return "never";
  const d = new Date(t).getTime();
  const ms = Date.now() - d;
  if (ms < 60_000) return "just now";
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)}m ago`;
  if (ms < 86_400_000) return `${Math.floor(ms / 3_600_000)}h ago`;
  return `${Math.floor(ms / 86_400_000)}d ago`;
}

export default function SettingsView() {
  const [perms, setPerms] = useState<CapturePermissionRow[]>([]);
  const [usage, setUsage] = useState<UsageStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const [forgotMsg, setForgotMsg] = useState<string | null>(null);
  const [sharePayload, setSharePayload] = useState<string | null>(null);

  async function reload() {
    setLoading(true);
    try {
      const [rows, u] = await Promise.all([listCapturePermissions(), getUsageStats()]);
      setPerms(rows);
      setUsage(u);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    reload();
  }, []);

  async function toggle(source: string, next: boolean) {
    try {
      await setCapturePermission(source, next);
      // Optimistic local update, then reload to pick up granted_at.
      setPerms((rows) => rows.map((r) => (r.source === source ? { ...r, enabled: next } : r)));
      await reload();
    } catch (e) {
      setError(String(e));
    }
  }

  async function forget(source: string) {
    if (!confirm(`Forget all captures from "${source}"?\n\nThis clears the audit counter and removes that source's entries from the recent ring buffer. The underlying entities remain (a future release will purge them too — tracked).`)) {
      return;
    }
    setForgetting(source);
    setForgotMsg(null);
    try {
      const r: ForgetSourceResult = await forgetCaptureSource(source);
      setForgotMsg(`Cleared ${r.traces_redacted} entries from recent.jsonl for "${source}".`);
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setForgetting(null);
    }
  }

  async function copyShare() {
    try {
      const payload = await getUsageSharePayload();
      setSharePayload(payload);
      // Best effort: copy to clipboard if available.
      try {
        await navigator.clipboard.writeText(payload);
      } catch {
        /* ignore — user can copy from the textarea */
      }
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="max-w-4xl mx-auto space-y-8">
      <header>
        <h2 className="text-2xl font-semibold text-tm-text">Settings & Privacy</h2>
        <p className="text-sm text-tm-muted mt-1">
          Everything below stays on this device. No telemetry, no upload — verify with{" "}
          <code className="text-tm-accent">tracemind capture audit-network</code>.
        </p>
      </header>

      {error && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200">
          {error}
        </div>
      )}

      {/* UI-13 — Capture permissions */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <div className="flex items-baseline justify-between mb-4">
          <div>
            <h3 className="text-lg font-medium text-tm-text">Capture sources</h3>
            <p className="text-xs text-tm-muted mt-0.5">
              Each source is opt-in. Toggle off any time. "Forget" clears the audit
              counter and the recent-event ring for that source.
            </p>
          </div>
          <button
            onClick={reload}
            className="text-xs text-tm-muted hover:text-tm-text transition-colors"
          >
            ↻ refresh
          </button>
        </div>

        {loading && <p className="text-sm text-tm-muted">Loading…</p>}

        {!loading && (
          <ul className="divide-y divide-tm-border/60">
            {perms.map((p) => (
              <li key={p.source} className="py-3 flex items-start gap-4">
                <div className="flex-1">
                  <div className="flex items-center gap-2">
                    <span className="font-medium text-tm-text capitalize">{p.source}</span>
                    {p.default_enabled && (
                      <span className="text-[10px] text-tm-muted uppercase tracking-wider bg-white/5 px-1.5 py-0.5 rounded">
                        on by default
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-tm-muted mt-0.5">{p.description}</p>
                  <p className="text-xs text-tm-muted mt-1">
                    {p.event_count.toLocaleString()} events captured · last{" "}
                    {relTime(p.last_event_at)}
                    {p.granted_at && (
                      <span> · granted {fmtTime(p.granted_at)}</span>
                    )}
                  </p>
                </div>
                <div className="flex flex-col items-end gap-2">
                  <ToggleSwitch
                    checked={p.enabled}
                    onChange={(next) => toggle(p.source, next)}
                  />
                  {p.event_count > 0 && (
                    <button
                      onClick={() => forget(p.source)}
                      disabled={forgetting === p.source}
                      className="text-[11px] text-tm-muted hover:text-red-400 transition-colors disabled:opacity-50"
                    >
                      {forgetting === p.source ? "forgetting…" : "forget all"}
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}

        {forgotMsg && (
          <p className="text-xs text-green-400 mt-3">{forgotMsg}</p>
        )}
      </section>

      {/* DP-3 — Usage stats */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <h3 className="text-lg font-medium text-tm-text mb-3">
          Your usage <span className="text-xs text-tm-muted font-normal">(local only)</span>
        </h3>
        {usage && (
          <div className="grid grid-cols-3 gap-3 text-sm">
            <Stat label="Queries" value={usage.total_queries.toLocaleString()} />
            <Stat label="👍 Helpful" value={usage.total_helpful.toLocaleString()} />
            <Stat label="👎 Not related" value={usage.total_negative.toLocaleString()} />
            <Stat label="First seen" value={fmtTime(usage.first_seen).split(",")[0] || "—"} />
            <Stat label="Last query" value={relTime(usage.last_query_at)} />
            <Stat label="Active days (90-d)" value={usage.active_days.length.toString()} />
          </div>
        )}
        <div className="mt-4 flex items-center gap-3">
          <button
            onClick={copyShare}
            className="text-xs px-3 py-1.5 bg-tm-accent/10 border border-tm-accent/30 text-tm-accent rounded hover:bg-tm-accent/20 transition-colors"
          >
            Copy usage JSON (for sharing with your DP contact)
          </button>
          <span className="text-xs text-tm-muted">
            Nothing leaves your machine. Paste it back manually if you'd like to share.
          </span>
        </div>
        {sharePayload && (
          <textarea
            readOnly
            value={sharePayload}
            className="mt-3 w-full h-40 bg-black/30 border border-tm-border rounded p-2 text-xs font-mono text-tm-muted"
            onFocus={(e) => e.currentTarget.select()}
          />
        )}
      </section>

      {/* Privacy invariants */}
      <section className="bg-tm-surface/50 border border-tm-border rounded-lg p-5">
        <h3 className="text-lg font-medium text-tm-text mb-2">Privacy invariants</h3>
        <ul className="text-sm text-tm-muted space-y-1.5">
          <li>· Every capture source is opt-in per-source. Default install: clipboard + shell + notes only.</li>
          <li>· Every captured memory carries a <code>source</code> tag visible in Traces.</li>
          <li>· "Forget this source" is one click and clears its audit counter + recent ring buffer.</li>
          <li>· No capture source transmits off-device. Audit: <code>tracemind capture audit-network</code>.</li>
          <li>· Model downloads are the only network calls, and only on first use (BGE, ColBERT, optional Tier-1 LLM).</li>
        </ul>
      </section>
    </div>
  );
}

function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: (next: boolean) => void }) {
  return (
    <button
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
        checked ? "bg-tm-accent" : "bg-white/10"
      }`}
      aria-pressed={checked}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${
          checked ? "translate-x-5" : "translate-x-1"
        }`}
      />
    </button>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-black/20 border border-tm-border/60 rounded p-2.5">
      <p className="text-[10px] uppercase tracking-wider text-tm-muted">{label}</p>
      <p className="text-base font-medium text-tm-text mt-0.5">{value}</p>
    </div>
  );
}
