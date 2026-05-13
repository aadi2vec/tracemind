import { useEffect, useState } from "react";
import BriefView from "./views/BriefView";
import Dashboard from "./views/Dashboard";
import QueryView from "./views/QueryView";
import TracesView from "./views/TracesView";
import IngestView from "./views/IngestView";
import GraphView from "./views/GraphView";
import ContextSwitcher from "./views/ContextSwitcher";
import SettingsView from "./views/SettingsView";
import OnboardingView from "./views/OnboardingView";
import CommitmentTimelineView from "./views/CommitmentTimelineView";
import CalibrationView from "./views/CalibrationView";
import MemoryGardenView from "./views/MemoryGardenView";
import ViewsView from "./views/ViewsView";
import { getUsageStats } from "./api";

type View =
  | "brief"
  | "dashboard"
  | "query"
  | "ingest"
  | "traces"
  | "graph"
  | "garden"
  | "views"
  | "commitments"
  | "calibration"
  | "settings"
  | "onboarding";

// Reason nav removed 2026-05-11 — reasoning primitives now surface as action
// cards in Dashboard (Next Actions) and inline in Query/Brief. See
// `tracemind_reason_to_proactive.md` memory.
const NAV_ITEMS: { id: View; label: string; icon: string }[] = [
  { id: "brief", label: "Brief", icon: "brief" },
  { id: "dashboard", label: "Dashboard", icon: "grid" },
  { id: "query", label: "Query", icon: "search" },
  { id: "ingest", label: "Ingest", icon: "plus" },
  { id: "graph", label: "Graph", icon: "graph" },
  { id: "garden", label: "Garden", icon: "garden" },
  { id: "views", label: "Views", icon: "views" },
  { id: "commitments", label: "Commitments", icon: "timeline" },
  { id: "calibration", label: "Calibration", icon: "gauge" },
  { id: "traces", label: "Traces", icon: "list" },
  { id: "settings", label: "Settings", icon: "settings" },
];

function NavIcon({ type }: { type: string }) {
  switch (type) {
    case "brief":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4" />
        </svg>
      );
    case "grid":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2V6zm10 0a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2V6zM4 16a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2v-2zm10 0a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2v-2z" />
        </svg>
      );
    case "search":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
      );
    case "plus":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
        </svg>
      );
    case "graph":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <circle cx="6" cy="6" r="2" strokeWidth={2} />
          <circle cx="18" cy="6" r="2" strokeWidth={2} />
          <circle cx="12" cy="18" r="2" strokeWidth={2} />
          <path strokeLinecap="round" strokeWidth={2} d="M8 7l4 9M16 7l-4 9M7.5 5.5h9" />
        </svg>
      );
    case "reason":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
        </svg>
      );
    case "list":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2" />
        </svg>
      );
    case "timeline":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 3v18M5 8h6M5 14h10M5 20h4" />
        </svg>
      );
    case "garden":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <circle cx="6" cy="10" r="2" strokeWidth={2} />
          <circle cx="12" cy="6" r="2" strokeWidth={2} />
          <circle cx="18" cy="10" r="2" strokeWidth={2} />
          <circle cx="9" cy="16" r="2" strokeWidth={2} />
          <circle cx="15" cy="16" r="2" strokeWidth={2} />
        </svg>
      );
    case "views":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h10M4 18h7" />
          <circle cx="18" cy="18" r="2" strokeWidth={2} />
        </svg>
      );
    case "gauge":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 12a9 9 0 1118 0M12 12l4-4" />
        </svg>
      );
    case "settings":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
        </svg>
      );
    default:
      return null;
  }
}

export default function App() {
  const [view, setView] = useState<View>("brief");
  const [firstRunChecked, setFirstRunChecked] = useState(false);

  // UI-8 — route first-run users to onboarding. We detect "first run"
  // as having no usage history yet (first_seen is null).
  useEffect(() => {
    getUsageStats()
      .then((u) => {
        if (!u.first_seen && u.total_queries === 0) {
          setView("onboarding");
        }
      })
      .catch(() => {
        /* fall through to brief — DP-3 failures shouldn't block UI */
      })
      .finally(() => setFirstRunChecked(true));
  }, []);

  // 2026-05-11 — Dashboard "Next Actions" cards dispatch a custom event.
  // Route by target_kind so commitment/contradiction cards open the
  // right surface.
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent).detail as { target_kind?: string } | undefined;
      if (!detail) return;
      if (detail.target_kind === "commitment") setView("commitments");
      else if (detail.target_kind === "contradiction") setView("brief");
      else if (detail.target_kind === "entity") setView("graph");
    };
    window.addEventListener("tm:next-action", handler);
    return () => window.removeEventListener("tm:next-action", handler);
  }, []);

  if (!firstRunChecked) {
    return (
      <div className="flex h-screen items-center justify-center bg-tm-bg">
        <div className="w-6 h-6 border-2 border-tm-accent border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (view === "onboarding") {
    return (
      <div className="h-screen overflow-y-auto bg-tm-bg p-6">
        <OnboardingView onComplete={() => setView("brief")} />
      </div>
    );
  }

  return (
    <div className="flex h-screen">
      {/* Sidebar */}
      <nav className="w-56 bg-tm-surface border-r border-tm-border flex flex-col">
        <div className="p-5 border-b border-tm-border">
          <h1 className="text-lg font-bold text-tm-accent">TraceMind</h1>
          <p className="text-xs text-tm-muted mt-0.5">Local Memory OS</p>
        </div>

        <div className="flex-1 py-3 overflow-y-auto">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              onClick={() => setView(item.id)}
              className={`w-full flex items-center gap-3 px-5 py-2.5 text-sm transition-colors ${
                view === item.id
                  ? "text-tm-accent bg-tm-accent/10 border-r-2 border-tm-accent"
                  : "text-tm-muted hover:text-tm-text hover:bg-white/5"
              }`}
            >
              <NavIcon type={item.icon} />
              {item.label}
            </button>
          ))}
        </div>

        <div className="p-3 border-t border-tm-border space-y-3">
          <div>
            <p className="text-[10px] uppercase tracking-wider text-tm-muted mb-1.5 px-1">Context</p>
            <ContextSwitcher />
          </div>
          <div className="flex items-center gap-2 px-1">
            <div className="w-2 h-2 rounded-full bg-tm-green animate-pulse" />
            <span className="text-xs text-tm-muted">Local only</span>
          </div>
        </div>
      </nav>

      {/* Main content */}
      <main className="flex-1 overflow-y-auto bg-tm-bg p-6">
        {view === "brief" && <BriefView />}
        {view === "dashboard" && <Dashboard />}
        {view === "query" && <QueryView />}
        {view === "ingest" && <IngestView />}
        {view === "graph" && <GraphView />}
        {view === "garden" && <MemoryGardenView />}
        {view === "views" && <ViewsView />}
        {view === "commitments" && <CommitmentTimelineView />}
        {view === "calibration" && <CalibrationView />}
        {view === "traces" && <TracesView />}
        {view === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
