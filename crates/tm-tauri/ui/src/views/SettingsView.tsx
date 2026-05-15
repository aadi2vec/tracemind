import { lazy, Suspense, useEffect, useState } from "react";
import {
  CapturePermissionRow,
  CleanupReport,
  ForgetSourceResult,
  LlmStatus,
  StorageStats,
  UsageStats,
  cleanEphemeralStorage,
  downloadLlm,
  forgetCaptureSource,
  getLlmStatus,
  getStorageStats,
  getUsageSharePayload,
  getUsageStats,
  listCapturePermissions,
  setCapturePermission,
  truncateTraces,
  vacuumStorage,
} from "../api";
import OntologyProposals from "./OntologyProposals";

// Schema editor is power-user-only. Lazy so first paint of Settings
// stays cheap and we don't load the full ontology editor unless asked.
const OntologyView = lazy(() => import("./OntologyView"));

// UI-12 + UI-13: settings + privacy panel. Per-source capture
// toggles, audit counters, "forget all from this source," + DP-3
// usage stats + opt-in share payload.

function fmtTime(t: string | null): string {
  if (!t) return "—";
  const d = new Date(t);
  if (Number.isNaN(d.getTime())) return t;
  return d.toLocaleString();
}

function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v >= 10 ? 1 : 2)} ${units[i]}`;
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

/** Dev-mode toggle exposes the Inspector sidebar entry. localStorage-backed
 *  so it survives reloads. Default OFF — power users opt in. */
const DEV_MODE_KEY = "tm:dev_mode";

export function readDevMode(): boolean {
  try {
    return localStorage.getItem(DEV_MODE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeDevMode(next: boolean) {
  try {
    if (next) localStorage.setItem(DEV_MODE_KEY, "1");
    else localStorage.removeItem(DEV_MODE_KEY);
  } catch {
    /* ignore — private browsing or storage quota */
  }
  // Broadcast so the shell can re-read without a full reload.
  window.dispatchEvent(new CustomEvent("tm:dev-mode-changed", { detail: next }));
}

export default function SettingsView() {
  const [perms, setPerms] = useState<CapturePermissionRow[]>([]);
  const [usage, setUsage] = useState<UsageStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const [forgotMsg, setForgotMsg] = useState<string | null>(null);
  const [sharePayload, setSharePayload] = useState<string | null>(null);
  const [devMode, setDevMode] = useState<boolean>(() => readDevMode());
  const [schemaOpen, setSchemaOpen] = useState(false);
  const [llm, setLlm] = useState<LlmStatus | null>(null);
  const [storage, setStorage] = useState<StorageStats | null>(null);
  const [storageBusy, setStorageBusy] = useState<string | null>(null);
  const [storageMsg, setStorageMsg] = useState<string | null>(null);
  const [llmDownloading, setLlmDownloading] = useState(false);
  const [llmMsg, setLlmMsg] = useState<string | null>(null);

  // Inline modal state — Tauri's WKWebView doesn't render window.confirm()
  // or window.prompt(), so we drive an in-app modal instead.
  const [confirmModal, setConfirmModal] = useState<{
    title: string;
    body: string;
    confirmLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
  } | null>(null);
  const [promptModal, setPromptModal] = useState<{
    title: string;
    body: string;
    initial: string;
    confirmLabel?: string;
    onSubmit: (value: string) => void;
  } | null>(null);
  const [promptValue, setPromptValue] = useState("");

  async function reloadStorage() {
    try {
      const s = await getStorageStats();
      setStorage(s);
    } catch (e) {
      setError(String(e));
    }
  }

  function reportToMsg(r: CleanupReport): string {
    const freed = r.bytes_freed >= 0
      ? `freed ${fmtBytes(r.bytes_freed)}`
      : `grew ${fmtBytes(-r.bytes_freed)}`;
    const rows = r.rows_deleted > 0 ? ` · removed ${r.rows_deleted.toLocaleString()} rows` : "";
    return `${r.action}: ${freed}${rows}`;
  }

  async function runVacuum() {
    setStorageBusy("vacuum");
    setStorageMsg(null);
    try {
      const r = await vacuumStorage();
      setStorageMsg(reportToMsg(r));
      await reloadStorage();
    } catch (e) {
      setError(String(e));
    } finally {
      setStorageBusy(null);
    }
  }

  async function runCleanEphemeralConfirmed() {
    setStorageBusy("ephemeral");
    setStorageMsg(null);
    try {
      const r = await cleanEphemeralStorage();
      setStorageMsg(reportToMsg(r));
      await reloadStorage();
    } catch (e) {
      setError(String(e));
    } finally {
      setStorageBusy(null);
    }
  }

  function runCleanEphemeral() {
    setConfirmModal({
      title: "Delete ephemeral / consolidated signals?",
      body:
        "Removes raw capture rows that have already been promoted into the graph (tier 4 = Ephemeral, or any consolidated signal above tier 1). Entities and triples are untouched. This cannot be undone.",
      confirmLabel: "Delete",
      danger: true,
      onConfirm: () => {
        setConfirmModal(null);
        void runCleanEphemeralConfirmed();
      },
    });
  }

  async function runTruncateTracesConfirmed(keep: number) {
    setStorageBusy("traces");
    setStorageMsg(null);
    try {
      const r = await truncateTraces(keep);
      setStorageMsg(reportToMsg(r));
      await reloadStorage();
    } catch (e) {
      setError(String(e));
    } finally {
      setStorageBusy(null);
    }
  }

  function runTruncateTraces() {
    setPromptValue("5000");
    setPromptModal({
      title: "Truncate trace log",
      body:
        "Keep how many of the most recent trace lines? Older entries will be discarded from traces.jsonl.",
      initial: "5000",
      confirmLabel: "Truncate",
      onSubmit: (raw) => {
        setPromptModal(null);
        const keep = Number.parseInt(raw, 10);
        if (!Number.isFinite(keep) || keep < 0) {
          setError("Keep-recent must be a non-negative integer.");
          return;
        }
        void runTruncateTracesConfirmed(keep);
      },
    });
  }

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
    getLlmStatus()
      .then(setLlm)
      .catch(() => setLlm(null));
    reloadStorage();
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

  async function forgetConfirmed(source: string) {
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

  function forget(source: string) {
    setConfirmModal({
      title: `Forget all captures from "${source}"?`,
      body:
        "Clears the audit counter and removes that source's entries from the recent ring buffer. The underlying entities remain (a future release will purge them too).",
      confirmLabel: "Forget",
      danger: true,
      onConfirm: () => {
        setConfirmModal(null);
        void forgetConfirmed(source);
      },
    });
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

      {/* LLM tier status */}
      {llm && (
        <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <h3 className="text-lg font-medium text-tm-text">LLM tier</h3>
                <span
                  className={`text-[10px] uppercase tracking-wider px-2 py-0.5 rounded border ${
                    llm.active
                      ? "bg-green-500/20 text-green-300 border-green-500/40"
                      : "bg-amber-500/20 text-amber-300 border-amber-500/40"
                  }`}
                >
                  {llm.active ? "Active" : "Heuristic only"}
                </span>
              </div>
              <p className="text-xs text-tm-muted mt-1 max-w-xl">
                Qwen 2.5 1.5B (Q4_K_M) drives rich triple extraction on the
                slow path when enabled. Tier 0 (extractive) is always the
                guaranteed fallback so queries never fail.
              </p>
              <ul className="text-xs text-tm-muted mt-2 space-y-0.5">
                <li>
                  Feature compiled in:{" "}
                  <span className={llm.feature_compiled ? "text-green-300" : "text-tm-text"}>
                    {llm.feature_compiled ? "yes" : "no (build with --features local-llm)"}
                  </span>
                </li>
                <li>
                  Weights present:{" "}
                  <span className={llm.weights_present ? "text-green-300" : "text-tm-text"}>
                    {llm.weights_present ? "yes" : "no — drop GGUF at path below"}
                  </span>
                </li>
                <li>
                  Path: <code className="text-[10px]">{llm.weights_path}</code>
                </li>
              </ul>
              {llm.feature_compiled && !llm.weights_present && (
                <div className="mt-3 flex items-center gap-3">
                  <button
                    onClick={async () => {
                      setLlmDownloading(true);
                      setLlmMsg(null);
                      try {
                        const next = await downloadLlm();
                        setLlm(next);
                        setLlmMsg("Weights installed. Tier 1 will activate on the next worker tick.");
                      } catch (e) {
                        setLlmMsg(`Download failed: ${e}`);
                      } finally {
                        setLlmDownloading(false);
                      }
                    }}
                    disabled={llmDownloading}
                    className="px-3 py-1.5 text-xs bg-tm-accent/20 text-tm-accent border border-tm-accent/40 rounded hover:bg-tm-accent/30 disabled:opacity-50"
                  >
                    {llmDownloading ? "Downloading… (~900 MB)" : "Download Tier-1 LLM (~900 MB)"}
                  </button>
                  {llmMsg && (
                    <span className="text-xs text-tm-muted">{llmMsg}</span>
                  )}
                </div>
              )}
              {llm.feature_compiled && llm.weights_present && llmMsg && (
                <p className="text-xs text-green-400 mt-2">{llmMsg}</p>
              )}
            </div>
          </div>
        </section>
      )}

      {/* Developer mode */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h3 className="text-lg font-medium text-tm-text">Developer mode</h3>
            <p className="text-xs text-tm-muted mt-1 max-w-xl">
              Reveals the <span className="text-tm-text">Inspector</span> sidebar
              entry — a 4-panel debug surface (brain snapshot, why-this-answer per
              trace, full entity context dump, raw per-layer browser). Off by
              default; turn on if you want to see how the system reasons.
            </p>
          </div>
          <ToggleSwitch
            checked={devMode}
            onChange={(next) => {
              setDevMode(next);
              writeDevMode(next);
            }}
          />
        </div>
      </section>

      {/* ONT-2 — Ontology proposals (statistical Object Type proposals
          from tm-reflect cluster signatures). Inline list, accept/reject
          buttons. Surfaces always — but quiet when there are none. */}
      <OntologyProposals />

      {/* Schema editor — power users only. Folded by default; opening
          loads the full ontology editor (the demoted OntologyView).
          Per the no-Foundry-UI rule, schema is internal infra. */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h3 className="text-lg font-medium text-tm-text">Schema (power users)</h3>
            <p className="text-xs text-tm-muted mt-1 max-w-xl">
              View and edit Object Types + Link Types — the typed schema
              that gates every new triple. Most users won't touch this.
              Statistical proposals appear above.
            </p>
          </div>
          <button
            onClick={() => setSchemaOpen((v) => !v)}
            className="text-xs px-3 py-1.5 border border-tm-border rounded text-tm-text hover:border-tm-accent"
          >
            {schemaOpen ? "Hide" : "Open"}
          </button>
        </div>
        {schemaOpen && (
          <div className="mt-4 pt-4 border-t border-tm-border">
            <Suspense fallback={<p className="text-sm text-tm-muted">Loading schema editor…</p>}>
              <OntologyView />
            </Suspense>
          </div>
        )}
      </section>

      {/* Storage — scan size + on-demand cleanup */}
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5">
        <div className="flex items-baseline justify-between mb-3">
          <div>
            <h3 className="text-lg font-medium text-tm-text">Storage</h3>
            <p className="text-xs text-tm-muted mt-0.5">
              Everything in <code className="text-tm-accent">{storage?.data_dir ?? "~/.tracemind"}</code>.
              Nothing leaves your device.
            </p>
          </div>
          <button
            onClick={reloadStorage}
            className="text-xs text-tm-muted hover:text-tm-text transition-colors"
          >
            ↻ rescan
          </button>
        </div>

        {!storage && <p className="text-sm text-tm-muted">Scanning…</p>}

        {storage && (
          <>
            <div className="grid grid-cols-3 gap-3 text-sm mb-4">
              <Stat label="Total on disk" value={fmtBytes(storage.total_bytes)} />
              <Stat label="Entities" value={storage.entity_count.toLocaleString()} />
              <Stat label="Triples" value={storage.triple_count.toLocaleString()} />
              <Stat label="Signals" value={storage.signal_count.toLocaleString()} />
              <Stat label="Ephemeral signals" value={storage.ephemeral_signal_count.toLocaleString()} />
              <Stat label="Trace lines" value={storage.trace_line_count.toLocaleString()} />
            </div>

            <details className="mb-4">
              <summary className="text-xs text-tm-muted cursor-pointer hover:text-tm-text">
                Per-file breakdown ({storage.files.length} files)
              </summary>
              <ul className="mt-2 text-xs font-mono text-tm-muted space-y-0.5 max-h-48 overflow-auto">
                {storage.files.map((f) => (
                  <li key={f.name} className="flex justify-between gap-4">
                    <span className="truncate">{f.name}</span>
                    <span className="shrink-0 tabular-nums">{fmtBytes(f.bytes)}</span>
                  </li>
                ))}
              </ul>
            </details>

            <div className="flex flex-wrap gap-2">
              <button
                onClick={runVacuum}
                disabled={storageBusy !== null}
                className="text-xs px-3 py-1.5 border border-tm-border rounded text-tm-text hover:border-tm-accent disabled:opacity-50"
                title="SQLite VACUUM on all DBs — reclaims freed pages."
              >
                {storageBusy === "vacuum" ? "Vacuuming…" : "Vacuum DBs"}
              </button>
              <button
                onClick={runCleanEphemeral}
                disabled={storageBusy !== null}
                className="text-xs px-3 py-1.5 border border-tm-border rounded text-tm-text hover:border-tm-accent disabled:opacity-50"
                title="Delete tier-4 (Ephemeral) + already-consolidated raw signals."
              >
                {storageBusy === "ephemeral" ? "Cleaning…" : "Clean ephemeral signals"}
              </button>
              <button
                onClick={runTruncateTraces}
                disabled={storageBusy !== null}
                className="text-xs px-3 py-1.5 border border-tm-border rounded text-tm-text hover:border-tm-accent disabled:opacity-50"
                title="Truncate traces.jsonl to the most recent N lines."
              >
                {storageBusy === "traces" ? "Truncating…" : "Truncate trace log…"}
              </button>
            </div>

            {storageMsg && (
              <p className="text-xs text-green-400 mt-3">{storageMsg}</p>
            )}
          </>
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

      {confirmModal && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
          onClick={() => setConfirmModal(null)}
        >
          <div
            className="bg-tm-surface border border-tm-border rounded-lg p-5 max-w-md w-[420px] shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-base font-semibold text-tm-text mb-2">
              {confirmModal.title}
            </h3>
            <p className="text-sm text-tm-muted whitespace-pre-line">
              {confirmModal.body}
            </p>
            <div className="flex justify-end gap-2 mt-5">
              <button
                onClick={() => setConfirmModal(null)}
                className="px-3 py-1.5 text-sm border border-tm-border rounded text-tm-text hover:bg-white/5"
              >
                Cancel
              </button>
              <button
                onClick={confirmModal.onConfirm}
                className={
                  confirmModal.danger
                    ? "px-3 py-1.5 text-sm bg-rose-500/20 text-rose-300 border border-rose-500/40 rounded hover:bg-rose-500/30"
                    : "px-3 py-1.5 text-sm bg-tm-accent/20 text-tm-accent border border-tm-accent/40 rounded hover:bg-tm-accent/30"
                }
              >
                {confirmModal.confirmLabel ?? "Confirm"}
              </button>
            </div>
          </div>
        </div>
      )}

      {promptModal && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
          onClick={() => setPromptModal(null)}
        >
          <div
            className="bg-tm-surface border border-tm-border rounded-lg p-5 max-w-md w-[420px] shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-base font-semibold text-tm-text mb-2">
              {promptModal.title}
            </h3>
            <p className="text-sm text-tm-muted">{promptModal.body}</p>
            <input
              autoFocus
              value={promptValue}
              onChange={(e) => setPromptValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  promptModal.onSubmit(promptValue);
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  setPromptModal(null);
                }
              }}
              className="mt-3 w-full bg-tm-bg border border-tm-border rounded px-2 py-1.5 text-sm text-tm-text focus:outline-none focus:border-tm-accent"
            />
            <div className="flex justify-end gap-2 mt-5">
              <button
                onClick={() => setPromptModal(null)}
                className="px-3 py-1.5 text-sm border border-tm-border rounded text-tm-text hover:bg-white/5"
              >
                Cancel
              </button>
              <button
                onClick={() => promptModal.onSubmit(promptValue)}
                className="px-3 py-1.5 text-sm bg-tm-accent/20 text-tm-accent border border-tm-accent/40 rounded hover:bg-tm-accent/30"
              >
                {promptModal.confirmLabel ?? "OK"}
              </button>
            </div>
          </div>
        </div>
      )}
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
