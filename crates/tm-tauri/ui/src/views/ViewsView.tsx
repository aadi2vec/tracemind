// LM-11e — Views surface: first-class sidebar listing of every saved
// splice. Splice (per-thread include/exclude state) used to be buried
// inside QueryView with no discovery path; this view promotes it so
// users can revisit named views and audit unnamed splices.
import { useEffect, useState } from "react";
import {
  clearThreadView,
  getEntityDrawer,
  listThreadViews,
  loadThreadView,
  type ThreadViewRow,
  type ThreadViewState,
} from "../api";

type ExpandedDetail = {
  state: ThreadViewState;
  includeNames: Map<string, string>;
  excludeNames: Map<string, string>;
};

export default function ViewsView() {
  const [rows, setRows] = useState<ThreadViewRow[] | null>(null);
  const [err, setErr] = useState<string>("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [detail, setDetail] = useState<ExpandedDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const refresh = () => {
    listThreadViews()
      .then((r) => {
        setRows(r);
        setErr("");
      })
      .catch((e) => setErr(String(e)));
  };

  useEffect(() => {
    refresh();
  }, []);

  const expand = async (threadId: string) => {
    if (expanded === threadId) {
      setExpanded(null);
      setDetail(null);
      return;
    }
    setExpanded(threadId);
    setDetail(null);
    setDetailLoading(true);
    try {
      const state = await loadThreadView(threadId);
      const includeNames = new Map<string, string>();
      const excludeNames = new Map<string, string>();
      // Resolve names in parallel; failures fall back to raw IDs.
      await Promise.all([
        ...state.include_ids.map(async (id) => {
          try {
            const d = await getEntityDrawer(id, 0);
            includeNames.set(id, d.header.name);
          } catch {
            /* leave unresolved */
          }
        }),
        ...state.exclude_ids.map(async (id) => {
          try {
            const d = await getEntityDrawer(id, 0);
            excludeNames.set(id, d.header.name);
          } catch {
            /* leave unresolved */
          }
        }),
      ]);
      setDetail({ state, includeNames, excludeNames });
    } catch (e) {
      setErr(String(e));
    } finally {
      setDetailLoading(false);
    }
  };

  const clear = async (threadId: string) => {
    if (!confirm("Clear this saved view? This cannot be undone.")) return;
    try {
      await clearThreadView(threadId);
      if (expanded === threadId) {
        setExpanded(null);
        setDetail(null);
      }
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  if (rows === null) {
    return (
      <div className="text-tm-muted text-sm">Loading saved views…</div>
    );
  }

  const named = rows.filter((r) => r.view_name);
  const unnamed = rows.filter((r) => !r.view_name);

  return (
    <div className="max-w-4xl space-y-6">
      <header>
        <h2 className="text-2xl font-bold text-tm-text mb-1">Views</h2>
        <p className="text-sm text-tm-muted">
          Saved splices — your per-thread include/exclude lenses. Named views
          surface first; unnamed splices live below as recent threads.
        </p>
      </header>

      {err && (
        <div className="border border-red-500/40 bg-red-500/10 text-red-300 text-sm rounded-md p-3">
          {err}
        </div>
      )}

      {rows.length === 0 && (
        <div className="border border-tm-border rounded-md p-6 text-tm-muted text-sm">
          No saved views yet. Run a query, toggle entities to include/exclude,
          and save the splice with a name to make it appear here.
        </div>
      )}

      {named.length > 0 && (
        <section>
          <h3 className="text-xs uppercase tracking-wider text-tm-muted mb-2">
            Named views ({named.length})
          </h3>
          <div className="space-y-2">
            {named.map((r) => (
              <ViewRow
                key={r.thread_id}
                row={r}
                expanded={expanded === r.thread_id}
                detail={expanded === r.thread_id ? detail : null}
                detailLoading={expanded === r.thread_id && detailLoading}
                onExpand={() => expand(r.thread_id)}
                onClear={() => clear(r.thread_id)}
              />
            ))}
          </div>
        </section>
      )}

      {unnamed.length > 0 && (
        <section>
          <h3 className="text-xs uppercase tracking-wider text-tm-muted mb-2">
            Recent splices ({unnamed.length})
          </h3>
          <div className="space-y-2">
            {unnamed.map((r) => (
              <ViewRow
                key={r.thread_id}
                row={r}
                expanded={expanded === r.thread_id}
                detail={expanded === r.thread_id ? detail : null}
                detailLoading={expanded === r.thread_id && detailLoading}
                onExpand={() => expand(r.thread_id)}
                onClear={() => clear(r.thread_id)}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function ViewRow({
  row,
  expanded,
  detail,
  detailLoading,
  onExpand,
  onClear,
}: {
  row: ThreadViewRow;
  expanded: boolean;
  detail: ExpandedDetail | null;
  detailLoading: boolean;
  onExpand: () => void;
  onClear: () => void;
}) {
  const title = row.view_name ?? `Thread ${row.thread_id.slice(0, 8)}…`;
  return (
    <div className="border border-tm-border rounded-md bg-tm-surface">
      <div className="flex items-center justify-between p-3">
        <button
          onClick={onExpand}
          className="flex-1 text-left flex items-center gap-3 hover:text-tm-accent transition-colors"
        >
          <svg
            className={`w-4 h-4 transition-transform ${expanded ? "rotate-90" : ""}`}
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
          </svg>
          <div>
            <div className="text-sm font-medium text-tm-text">{title}</div>
            <div className="text-xs text-tm-muted font-mono mt-0.5">
              {row.thread_id}
            </div>
          </div>
        </button>
        <div className="flex items-center gap-3 ml-3">
          {row.include_count > 0 && (
            <span className="text-xs text-tm-green">
              +{row.include_count}
            </span>
          )}
          {row.exclude_count > 0 && (
            <span className="text-xs text-red-400">
              −{row.exclude_count}
            </span>
          )}
          <button
            onClick={onClear}
            className="text-xs text-tm-muted hover:text-red-400 px-2 py-1 transition-colors"
            title="Clear this saved view"
          >
            Clear
          </button>
        </div>
      </div>
      {expanded && (
        <div className="border-t border-tm-border p-3 space-y-3 bg-tm-bg/40">
          {detailLoading && (
            <div className="text-xs text-tm-muted">Loading entity names…</div>
          )}
          {detail && (
            <>
              {detail.state.include_ids.length > 0 && (
                <div>
                  <div className="text-[10px] uppercase tracking-wider text-tm-muted mb-1">
                    Include ({detail.state.include_ids.length})
                  </div>
                  <ul className="space-y-0.5">
                    {detail.state.include_ids.map((id) => (
                      <li key={id} className="text-xs text-tm-text">
                        <span className="text-tm-green">+ </span>
                        {detail.includeNames.get(id) ?? (
                          <span className="font-mono text-tm-muted">{id}</span>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {detail.state.exclude_ids.length > 0 && (
                <div>
                  <div className="text-[10px] uppercase tracking-wider text-tm-muted mb-1">
                    Exclude ({detail.state.exclude_ids.length})
                  </div>
                  <ul className="space-y-0.5">
                    {detail.state.exclude_ids.map((id) => (
                      <li key={id} className="text-xs text-tm-text">
                        <span className="text-red-400">− </span>
                        {detail.excludeNames.get(id) ?? (
                          <span className="font-mono text-tm-muted">{id}</span>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {detail.state.include_ids.length === 0 &&
                detail.state.exclude_ids.length === 0 && (
                  <div className="text-xs text-tm-muted">Empty splice.</div>
                )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
