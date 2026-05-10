import { useState } from "react";
import BriefView from "./views/BriefView";
import Dashboard from "./views/Dashboard";
import QueryView from "./views/QueryView";
import TracesView from "./views/TracesView";
import IngestView from "./views/IngestView";
import GraphView from "./views/GraphView";
import ReasonView from "./views/ReasonView";

type View = "brief" | "dashboard" | "query" | "ingest" | "traces" | "graph" | "reason";

const NAV_ITEMS: { id: View; label: string; icon: string }[] = [
  { id: "brief", label: "Brief", icon: "brief" },
  { id: "dashboard", label: "Dashboard", icon: "grid" },
  { id: "query", label: "Query", icon: "search" },
  { id: "ingest", label: "Ingest", icon: "plus" },
  { id: "graph", label: "Graph", icon: "graph" },
  { id: "reason", label: "Reason", icon: "reason" },
  { id: "traces", label: "Traces", icon: "list" },
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
    default:
      return null;
  }
}

export default function App() {
  const [view, setView] = useState<View>("brief");

  return (
    <div className="flex h-screen">
      {/* Sidebar */}
      <nav className="w-56 bg-tm-surface border-r border-tm-border flex flex-col">
        <div className="p-5 border-b border-tm-border">
          <h1 className="text-lg font-bold text-tm-accent">TraceMind</h1>
          <p className="text-xs text-tm-muted mt-0.5">Local Memory OS</p>
        </div>

        <div className="flex-1 py-3">
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

        <div className="p-4 border-t border-tm-border">
          <div className="flex items-center gap-2">
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
        {view === "reason" && <ReasonView />}
        {view === "traces" && <TracesView />}
      </main>
    </div>
  );
}
