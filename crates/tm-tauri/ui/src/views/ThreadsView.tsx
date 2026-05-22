// Sprint GRAPH — Threads surface.
//
// Threads are first-class composable graphs: each one is a sliver of memory
// scoped to a particular AI session / task. This view lets the user list
// open + closed threads, start new ones, end active ones, and inspect the
// materialized graph (event nodes + entities + topic clusters +
// commitments + capture signals) that the thread anchors.
import { useEffect, useState } from "react";
import {
  threadStart,
  threadEnd,
  threadsList,
  threadMaterialize,
  type ThreadDto,
  type ThreadGraphDto,
  type ThreadSource,
} from "../api";

const SOURCES: ThreadSource[] = [
  "tracemind",
  "claude",
  "cursor",
  "goose",
  "mcp",
  "other",
];

export default function ThreadsView() {
  const [threads, setThreads] = useState<ThreadDto[] | null>(null);
  const [err, setErr] = useState<string>("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [detail, setDetail] = useState<ThreadGraphDto | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const [newTitle, setNewTitle] = useState("");
  const [newSource, setNewSource] = useState<ThreadSource>("tracemind");
  const [creating, setCreating] = useState(false);

  const refresh = () => {
    threadsList(100)
      .then((rows) => {
        setThreads(rows);
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
      const g = await threadMaterialize(threadId);
      setDetail(g);
    } catch (e) {
      setErr(String(e));
    } finally {
      setDetailLoading(false);
    }
  };

  const create = async () => {
    if (!newTitle.trim()) return;
    setCreating(true);
    try {
      await threadStart(newTitle.trim(), newSource);
      setNewTitle("");
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setCreating(false);
    }
  };

  const end = async (threadId: string) => {
    try {
      await threadEnd(threadId);
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="max-w-5xl">
      <div className="mb-6">
        <h2 className="text-2xl font-semibold text-tm-text">Threads</h2>
        <p className="text-sm text-tm-muted mt-1">
          Each thread is a composable slice of memory tied to a session or
          task. End a thread when its work is done — you can still materialize
          its graph later.
        </p>
      </div>

      {err && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-red/10 border border-tm-red/30 text-tm-red text-sm">
          {err}
        </div>
      )}

      <div className="mb-6 p-4 rounded-lg border border-tm-border bg-tm-surface">
        <div className="text-xs uppercase tracking-wider text-tm-muted mb-2">
          Start a new thread
        </div>
        <div className="flex gap-2">
          <input
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
            placeholder="e.g. Sprint GRAPH wrap-up"
            className="flex-1 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent"
            onKeyDown={(e) => {
              if (e.key === "Enter") create();
            }}
          />
          <select
            value={newSource}
            onChange={(e) => setNewSource(e.target.value as ThreadSource)}
            className="px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
          >
            {SOURCES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
          <button
            onClick={create}
            disabled={creating || !newTitle.trim()}
            className="px-4 py-2 bg-tm-accent text-black rounded text-sm font-medium hover:bg-tm-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {creating ? "…" : "Start"}
          </button>
        </div>
      </div>

      {threads === null ? (
        <div className="text-sm text-tm-muted">Loading…</div>
      ) : threads.length === 0 ? (
        <div className="text-sm text-tm-muted">
          No threads yet. Starting one above creates a composable container
          for the next session's memory.
        </div>
      ) : (
        <div className="space-y-2">
          {threads.map((t) => {
            const open = t.ended_at === null;
            const isExpanded = expanded === t.id;
            return (
              <div
                key={t.id}
                className="rounded border border-tm-border bg-tm-surface overflow-hidden"
              >
                <div className="flex items-center gap-3 px-4 py-3">
                  <button
                    onClick={() => expand(t.id)}
                    className="flex-1 text-left"
                  >
                    <div className="flex items-center gap-2">
                      <span
                        className={`text-xs px-1.5 py-0.5 rounded ${
                          open
                            ? "bg-tm-green/15 text-tm-green"
                            : "bg-tm-muted/15 text-tm-muted"
                        }`}
                      >
                        {open ? "open" : "ended"}
                      </span>
                      <span className="text-xs uppercase tracking-wider text-tm-muted">
                        {t.source}
                      </span>
                      <span className="text-sm font-medium text-tm-text">
                        {t.title}
                      </span>
                    </div>
                    <div className="text-xs text-tm-muted mt-0.5 font-mono">
                      {t.id.slice(0, 8)} · started {fmt(t.started_at)}
                      {t.ended_at ? ` · ended ${fmt(t.ended_at)}` : ""}
                    </div>
                  </button>
                  {open && (
                    <button
                      onClick={() => end(t.id)}
                      className="px-2.5 py-1 text-xs rounded border border-tm-border text-tm-muted hover:text-tm-text hover:border-tm-accent"
                    >
                      End
                    </button>
                  )}
                </div>
                {isExpanded && (
                  <div className="border-t border-tm-border px-4 py-3 bg-tm-bg/40">
                    {detailLoading ? (
                      <div className="text-xs text-tm-muted">
                        Materializing…
                      </div>
                    ) : detail && detail.thread_id === t.id ? (
                      <ThreadGraphPanel graph={detail} />
                    ) : null}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ThreadGraphPanel({ graph }: { graph: ThreadGraphDto }) {
  const rows: Array<[string, number]> = [
    ["Event nodes", graph.event_node_ids.length],
    ["Entities", graph.entity_ids.length],
    ["Topic clusters", graph.topic_clusters.length],
    ["Commitments", graph.commitment_ids.length],
    ["Capture signals", graph.capture_signal_ids.length],
  ];
  const allZero = rows.every(([, n]) => n === 0);
  return (
    <div className="space-y-2">
      <div className="grid grid-cols-5 gap-3 text-xs">
        {rows.map(([label, n]) => (
          <div
            key={label}
            className="px-3 py-2 rounded bg-tm-surface border border-tm-border"
          >
            <div className="text-tm-muted uppercase tracking-wider text-[10px]">
              {label}
            </div>
            <div className="text-tm-text text-lg font-semibold mt-0.5">{n}</div>
          </div>
        ))}
      </div>
      {allZero && (
        <div className="text-[11px] text-tm-muted leading-snug">
          Thread is empty because the ingest → event-graph wiring is part
          of the EVG sprint and not live in this build. Captures and
          queries are not yet associated with threads.
        </div>
      )}
    </div>
  );
}

function fmt(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}
