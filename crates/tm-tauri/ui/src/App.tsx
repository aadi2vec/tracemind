import { useEffect, useState } from "react";
import HomeView from "./views/HomeView";
import BriefView from "./views/BriefView";
import Dashboard from "./views/Dashboard";
import QueryView from "./views/QueryView";
import TracesView from "./views/TracesView";
import IngestView from "./views/IngestView";
import IngestionReviewView from "./views/IngestionReviewView";
import GraphView from "./views/GraphView";
import ContextSwitcher from "./views/ContextSwitcher";
import SettingsView, { readDevMode } from "./views/SettingsView";
import OnboardingView from "./views/OnboardingView";
import CommitmentTimelineView from "./views/CommitmentTimelineView";
import CalibrationView from "./views/CalibrationView";
import MemoryGardenView from "./views/MemoryGardenView";
import ViewsView from "./views/ViewsView";
import ContextDashboardView from "./views/ContextDashboardView";
import InspectorView from "./views/InspectorView";
import ThreadsView from "./views/ThreadsView";
import ComposerView from "./views/ComposerView";
import EventGraphView from "./views/EventGraphView";
import LedgerView from "./views/LedgerView";
// OntologyView demoted 2026-05-13 — now imported only from SettingsView
// behind the Schema sub-panel (power users). Not in primary nav.
import {
  getUsageStats,
  demoTourGet,
  demoTourBegin,
  demoTourEnd,
  demoTourMark,
  type DemoTourState,
} from "./api";

type View =
  | "home"
  | "brief"
  | "dashboard"
  | "query"
  | "ingest"
  | "review"
  | "traces"
  | "graph"
  | "garden"
  | "context"
  | "views"
  | "commitments"
  | "ledger"
  | "calibration"
  | "threads"
  | "composer"
  | "events"
  | "inspector"
  | "settings"
  | "onboarding";

// Reason nav removed 2026-05-11 — reasoning primitives now surface as action
// cards in Dashboard (Next Actions) and inline in Query/Brief. See
// `tracemind_reason_to_proactive.md` memory.
//
// Inspector is gated behind Settings → Developer mode (off by default). We
// build NAV_ITEMS dynamically so toggling dev mode hides/shows the entry
// without a reload.
type NavItem = { id: View; label: string; icon: string };

// 2026-05-14 — sidebar trimmed from 15 items to 6 primary surfaces.
// Power users opt into the rest via Settings → Developer mode. The
// six items below trace the daily flow: see what's happening (Brief)
// → splice context for the next AI window (Composer) → search the
// graph (Query) → visualize (Graph) → write (Ingest) → tune
// (Settings). Composer is the wedge per `tracemind_composition_layer`.
// 2026-07-24 — nav collapsed to the sellable four. Home is the daily
// surface; Ask searches memory; Review triages captures; Settings tunes.
// Everything else moves under Dev mode. The prior surfaces (Brief,
// Composer, Graph, etc.) still exist and are one keystroke away for
// power users, but a first-time visitor sees a clean four-item shell.
const PRIMARY_NAV_ITEMS: NavItem[] = [
  { id: "home", label: "Home", icon: "brief" },
  { id: "query", label: "Ask", icon: "search" },
  { id: "review", label: "Review", icon: "list" },
  { id: "settings", label: "Settings", icon: "settings" },
];

// Hidden under dev mode. These are real working surfaces — we just
// keep them off the default sidebar so newcomers see one obvious
// path. Inspector stays at the end of the advanced list per its
// existing convention.
const ADVANCED_NAV_ITEMS: NavItem[] = [
  { id: "brief", label: "Brief", icon: "brief" },
  { id: "composer", label: "Composer", icon: "graph" },
  { id: "graph", label: "Graph", icon: "graph" },
  { id: "ingest", label: "Ingest", icon: "plus" },
  { id: "dashboard", label: "Dashboard", icon: "grid" },
  { id: "threads", label: "Threads", icon: "timeline" },
  { id: "views", label: "Views", icon: "views" },
  { id: "context", label: "Context", icon: "context" },
  { id: "garden", label: "Garden", icon: "garden" },
  { id: "events", label: "Events", icon: "graph" },
  { id: "ledger", label: "Ledger", icon: "timeline" },
  { id: "commitments", label: "Commitments", icon: "timeline" },
  { id: "calibration", label: "Calibration", icon: "gauge" },
  { id: "traces", label: "Traces", icon: "list" },
];

const INSPECTOR_NAV_ITEM: NavItem = {
  id: "inspector",
  label: "Inspector",
  icon: "inspect",
};

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
    case "context":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <circle cx="12" cy="12" r="3" strokeWidth={2} />
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" />
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
    case "inspect":
      return (
        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <circle cx="11" cy="11" r="6" strokeWidth={2} />
          <path strokeLinecap="round" strokeWidth={2} d="M15 15l5 5M11 8v6M8 11h6" />
        </svg>
      );
    default:
      return null;
  }
}

export default function App() {
  const [view, setView] = useState<View>("home");
  const [firstRunChecked, setFirstRunChecked] = useState(false);
  const [devMode, setDevMode] = useState<boolean>(() => readDevMode());
  const [tour, setTour] = useState<{ on: boolean; dwellMs: number }>({
    on: false,
    dwellMs: 6000,
  });

  // SettingsView fires `tm:dev-mode-changed` whenever the toggle flips —
  // we update local state so the sidebar shows/hides the Inspector entry
  // immediately.
  useEffect(() => {
    const handler = (e: Event) => {
      setDevMode(Boolean((e as CustomEvent).detail));
    };
    window.addEventListener("tm:dev-mode-changed", handler);
    return () => window.removeEventListener("tm:dev-mode-changed", handler);
  }, []);

  // An active tour implies dev mode: there is nothing to tour unless the
  // advanced surfaces are in the sidebar. This is also what makes the tour
  // reachable headlessly — dev mode lives in localStorage, which a
  // recording harness outside the webview cannot write, whereas the tour
  // flag is backend-persisted.
  const advancedVisible = devMode || tour.on;

  // In normal mode: 6 primary items. In dev mode: primary items +
  // advanced surfaces + Inspector, with Settings always last.
  const navItems = advancedVisible
    ? [
        ...PRIMARY_NAV_ITEMS.slice(0, -1), // everything except Settings
        ...ADVANCED_NAV_ITEMS,
        INSPECTOR_NAV_ITEM,
        PRIMARY_NAV_ITEMS[PRIMARY_NAV_ITEMS.length - 1], // Settings pinned to bottom
      ]
    : PRIMARY_NAV_ITEMS;

  // If dev mode is turned off while the user is on a hidden surface,
  // route them back to Home — otherwise the sidebar wouldn't show
  // their current view and they'd be stranded.
  useEffect(() => {
    if (advancedVisible) return;
    const visible = new Set<View>(PRIMARY_NAV_ITEMS.map((n) => n.id));
    visible.add("onboarding");
    if (!visible.has(view)) setView("home");
  }, [advancedVisible, view]);

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
        /* fall through to home — DP-3 failures shouldn't block UI */
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
      else if (detail.target_kind === "context") setView("context");
    };
    window.addEventListener("tm:next-action", handler);
    return () => window.removeEventListener("tm:next-action", handler);
  }, []);

  // Demo tour — walk every surface on a timer so the GUI walkthrough can be
  // recorded without a human clicking the sidebar 18 times. Read from the
  // backend (not localStorage) so a recording harness can enable it by
  // writing $TM_DATA_DIR/demo_tour.json; see Rust `DemoTourState`.
  useEffect(() => {
    const apply = (t: DemoTourState) =>
      setTour({ on: t.enabled, dwellMs: (t.dwell_secs || 6) * 1000 });
    demoTourGet()
      .then(apply)
      .catch(() => {
        /* file absent / older backend → tour stays off */
      });
    const handler = (e: Event) => {
      const d = (e as CustomEvent).detail as DemoTourState | undefined;
      if (d) apply(d);
    };
    window.addEventListener("tm:demo-tour-changed", handler);
    return () => window.removeEventListener("tm:demo-tour-changed", handler);
  }, []);

  // The walk itself. `navItems` is intentionally not a dependency — it's a
  // fresh array each render, and it's fully derived from `advancedVisible`,
  // which is.
  useEffect(() => {
    if (!tour.on || !advancedVisible || !firstRunChecked) return;
    const order: View[] = navItems.map((n) => n.id);
    let i = 0;
    setView(order[0]);

    // Fullscreen for the duration, so a recording harness captures exactly the
    // app — no desktop, no dock, and crucially none of the user's own files.
    // Driven through a Rust command rather than `@tauri-apps/api/window`,
    // which the Tauri v2 capability system denies here (no `capabilities/`).
    demoTourBegin().catch(() => {
      /* fullscreen is a nicety — the tour still walks without it */
    });
    demoTourMark(order[0]).catch(() => {});

    const timer = setInterval(() => {
      i += 1;
      if (i >= order.length) {
        clearInterval(timer);
        demoTourEnd().catch(() => {});
        return;
      }
      setView(order[i]);
      demoTourMark(order[i]).catch(() => {});
    }, tour.dwellMs);
    return () => {
      clearInterval(timer);
      demoTourEnd().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tour.on, tour.dwellMs, advancedVisible, firstRunChecked]);

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
        <OnboardingView onComplete={() => setView("home")} />
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
          {navItems.map((item) => (
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
            <p className="text-[10px] uppercase tracking-wider text-tm-muted mb-1.5 px-1">Active graph</p>
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
        {view === "home" && (
          <HomeView
            onOpenAsk={() => setView("query")}
            onOpenReview={() => setView("review")}
          />
        )}
        {view === "brief" && <BriefView onOpenLedger={() => setView("ledger")} />}
        {view === "dashboard" && <Dashboard />}
        {view === "query" && <QueryView />}
        {view === "ingest" && <IngestView />}
        {view === "review" && <IngestionReviewView />}
        {view === "graph" && <GraphView />}
        {view === "garden" && <MemoryGardenView />}
        {view === "context" && <ContextDashboardView />}
        {view === "views" && <ViewsView />}
        {view === "threads" && <ThreadsView />}
        {view === "composer" && <ComposerView />}
        {view === "events" && <EventGraphView />}
        {view === "ledger" && <LedgerView />}
        {view === "commitments" && <CommitmentTimelineView />}
        {view === "calibration" && <CalibrationView />}
        {view === "traces" && <TracesView />}
        {view === "inspector" && <InspectorView />}
        {view === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
