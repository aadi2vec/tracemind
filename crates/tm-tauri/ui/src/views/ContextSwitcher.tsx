// Sprint D / UI-3 — active-context badge + dropdown.
//
// Renders the currently-active context (or "all contexts") and lets the
// user switch with a single click. Wraps the `cmd_context_*` IPC calls
// added in the same sprint. Lives in the sidebar (above the "Local only"
// indicator) so context is visible from every view.

import { useEffect, useState } from "react";
import {
  ContextInfo,
  clearContext,
  createContext as createCtx,
  currentContext,
  listContexts,
  useContext as useCtx,
} from "../api";

export default function ContextSwitcher() {
  const [contexts, setContexts] = useState<ContextInfo[]>([]);
  const [active, setActive] = useState<ContextInfo | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [newName, setNewName] = useState("");

  async function refresh() {
    try {
      const [all, cur] = await Promise.all([listContexts(), currentContext()]);
      setContexts(all);
      setActive(cur);
    } catch (e) {
      // Surface but don't crash — the header should be best-effort.
      console.error("context refresh failed:", e);
    }
  }

  useEffect(() => {
    refresh();
    // 2026-05-11 — QueryView (and others) may switch context via useContext()
    // without going through this dropdown. Listen for a global event so the
    // badge stays in sync.
    const handler = () => {
      refresh();
    };
    window.addEventListener("tm:context-changed", handler);
    return () => window.removeEventListener("tm:context-changed", handler);
  }, []);

  async function switchTo(name: string) {
    setBusy(true);
    try {
      await useCtx(name);
      await refresh();
      window.dispatchEvent(new CustomEvent("tm:context-changed"));
      setOpen(false);
    } finally {
      setBusy(false);
    }
  }

  async function clearActive() {
    setBusy(true);
    try {
      await clearContext();
      await refresh();
      window.dispatchEvent(new CustomEvent("tm:context-changed"));
      setOpen(false);
    } finally {
      setBusy(false);
    }
  }

  async function handleCreate() {
    const name = newName.trim();
    if (!name) return;
    setBusy(true);
    try {
      await createCtx(name);
      setNewName("");
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  const label = active ? active.name : "all contexts";

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        disabled={busy}
        className="w-full flex items-center justify-between gap-2 px-3 py-1.5 rounded text-xs bg-tm-bg border border-tm-border hover:border-tm-accent transition-colors"
        title="Switch active context"
      >
        <span className="flex items-center gap-2 truncate">
          <span className={`w-1.5 h-1.5 rounded-full ${active ? "bg-tm-accent" : "bg-tm-muted"}`} />
          <span className="truncate">{label}</span>
        </span>
        <svg className="w-3 h-3 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && (
        <div className="absolute left-0 right-0 bottom-full mb-1 z-10 bg-tm-surface border border-tm-border rounded shadow-lg max-h-80 overflow-y-auto">
          <button
            onClick={clearActive}
            disabled={busy}
            className={`w-full text-left px-3 py-2 text-xs hover:bg-white/5 ${
              active === null ? "text-tm-accent" : "text-tm-muted"
            }`}
          >
            all contexts (no scope)
          </button>
          {contexts.map((c) => (
            <button
              key={c.id}
              onClick={() => switchTo(c.name)}
              disabled={busy}
              className={`w-full text-left px-3 py-2 text-xs hover:bg-white/5 truncate ${
                c.is_active ? "text-tm-accent" : "text-tm-text"
              }`}
              title={c.tags}
            >
              {c.is_active ? "● " : "○ "}
              {c.name}
            </button>
          ))}
          <div className="border-t border-tm-border px-2 py-2 flex gap-1">
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="new context name"
              className="flex-1 min-w-0 px-2 py-1 text-xs bg-tm-bg border border-tm-border rounded"
            />
            <button
              onClick={handleCreate}
              disabled={busy || !newName.trim()}
              className="px-2 py-1 text-xs bg-tm-accent/20 text-tm-accent rounded hover:bg-tm-accent/30 disabled:opacity-50"
            >
              +
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
