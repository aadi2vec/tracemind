import { useState } from "react";
import BrainSnapshotPanel from "./inspector/BrainSnapshotPanel";
import WhyThisAnswerPanel from "./inspector/WhyThisAnswerPanel";
import LayerBrowserPanel from "./inspector/LayerBrowserPanel";
import ContextDashboardView from "./ContextDashboardView";

// Inspector — power-user surface gated behind Settings → "Developer mode".
// Four panels, one tab strip:
//   1. Snapshot — single-page health dump of every layer
//   2. Why this answer — per-trace deep dive (planner, arm scores, entities)
//   3. Context — full per-entity context dump (re-uses ContextDashboardView)
//   4. Layers — raw per-layer browser (vector/graph/episodic/bandit/governance/reason)
//
// Not a scaffold: every panel calls real backend commands and renders real
// state. No mocks, no stubs.

type Tab = "snapshot" | "why" | "context" | "layers";

const TABS: { id: Tab; label: string; hint: string }[] = [
  { id: "snapshot", label: "Brain snapshot", hint: "live health of every layer" },
  { id: "why", label: "Why this answer", hint: "per-trace planner + bandit + entities" },
  { id: "context", label: "Entity context", hint: "everything we know about one node" },
  { id: "layers", label: "Layer browser", hint: "raw per-layer state" },
];

export default function InspectorView() {
  const [tab, setTab] = useState<Tab>("snapshot");

  return (
    <div className="max-w-6xl mx-auto space-y-5">
      <header>
        <div className="flex items-baseline justify-between">
          <h2 className="text-2xl font-semibold text-tm-text">Inspector</h2>
          <p className="text-xs text-tm-muted">
            Developer surface · toggle off in <span className="text-tm-text">Settings → Developer mode</span>
          </p>
        </div>
        <p className="text-sm text-tm-muted mt-1">
          Cross-section the brain. Every number is computed live from
          <code className="text-tm-accent ml-1">~/.tracemind/</code>; nothing here is cached or precomputed.
        </p>
      </header>

      <nav className="flex items-center border-b border-tm-border">
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={`px-4 py-2 text-sm transition-colors border-b-2 -mb-px ${
              tab === t.id
                ? "border-tm-accent text-tm-accent"
                : "border-transparent text-tm-muted hover:text-tm-text"
            }`}
            title={t.hint}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <div>
        {tab === "snapshot" && <BrainSnapshotPanel />}
        {tab === "why" && <WhyThisAnswerPanel />}
        {tab === "context" && <ContextDashboardView />}
        {tab === "layers" && <LayerBrowserPanel />}
      </div>
    </div>
  );
}
