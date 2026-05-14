// Sprint GRAPH — Composer surface.
//
// The composer is the user-facing edge of the graph algebra. It lets the
// user pick two threads, apply a set-op (union / intersect / diff), and
// preview the materialized result. From there they can "Send to Window 4"
// — i.e. export the composed graph as a portable JSON snapshot that can
// be handed to any other AI surface.
import { useEffect, useState } from "react";
import {
  composeSimple,
  exportPortable,
  threadsList,
  type SetOp,
  type ThreadDto,
  type ThreadGraphDto,
  type PortableExportResp,
} from "../api";

const OPS: Array<{ id: SetOp; label: string; hint: string }> = [
  { id: "union", label: "Union (∪)", hint: "Everything from both threads" },
  {
    id: "intersect",
    label: "Intersect (∩)",
    hint: "Only nodes both threads share",
  },
  {
    id: "diff",
    label: "Difference (\\)",
    hint: "Nodes in the left thread, but not the right",
  },
];

export default function ComposerView() {
  const [threads, setThreads] = useState<ThreadDto[] | null>(null);
  const [left, setLeft] = useState<string>("");
  const [right, setRight] = useState<string>("");
  const [op, setOp] = useState<SetOp>("union");
  const [err, setErr] = useState<string>("");

  const [result, setResult] = useState<ThreadGraphDto | null>(null);
  const [composing, setComposing] = useState(false);

  const [exported, setExported] = useState<PortableExportResp | null>(null);
  const [exporting, setExporting] = useState(false);

  useEffect(() => {
    threadsList(100)
      .then((rows) => {
        setThreads(rows);
        setErr("");
        if (rows.length > 0) {
          setLeft(rows[0].id);
          setRight(rows.length > 1 ? rows[1].id : rows[0].id);
        }
      })
      .catch((e) => setErr(String(e)));
  }, []);

  const run = async () => {
    if (!left || !right) return;
    setComposing(true);
    setExported(null);
    try {
      const g = await composeSimple(op, left, right);
      setResult(g);
      setErr("");
    } catch (e) {
      setErr(String(e));
      setResult(null);
    } finally {
      setComposing(false);
    }
  };

  const sendToWindow4 = async (writeToDisk: boolean) => {
    if (!left || !right) return;
    setExporting(true);
    try {
      const resp = await exportPortable(
        {
          node: "set_op",
          op,
          left: { node: "thread", thread_id: left },
          right: { node: "thread", thread_id: right },
        },
        writeToDisk,
      );
      setExported(resp);
      setErr("");
    } catch (e) {
      setErr(String(e));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="max-w-5xl">
      <div className="mb-6">
        <h2 className="text-2xl font-semibold text-tm-text">Composer</h2>
        <p className="text-sm text-tm-muted mt-1">
          Combine two threads with a set-op, then send the result anywhere —
          the export is a portable graph that any AI surface can read.
        </p>
      </div>

      {err && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-red/10 border border-tm-red/30 text-tm-red text-sm">
          {err}
        </div>
      )}

      {threads === null ? (
        <div className="text-sm text-tm-muted">Loading threads…</div>
      ) : threads.length === 0 ? (
        <div className="text-sm text-tm-muted">
          You need at least one thread before you can compose. Start one in
          the Threads view.
        </div>
      ) : (
        <>
          <div className="grid grid-cols-3 gap-3 mb-4">
            <ThreadPicker
              label="Left"
              threads={threads}
              value={left}
              onChange={setLeft}
            />
            <OpPicker value={op} onChange={setOp} />
            <ThreadPicker
              label="Right"
              threads={threads}
              value={right}
              onChange={setRight}
            />
          </div>

          <div className="flex gap-2 mb-6">
            <button
              onClick={run}
              disabled={composing || !left || !right}
              className="px-4 py-2 bg-tm-accent text-black rounded text-sm font-medium hover:bg-tm-accent/90 disabled:opacity-50"
            >
              {composing ? "Composing…" : "Compose"}
            </button>
            <button
              onClick={() => sendToWindow4(false)}
              disabled={exporting || !left || !right}
              className="px-4 py-2 border border-tm-border text-tm-text rounded text-sm hover:border-tm-accent disabled:opacity-50"
              title="Build a portable JSON graph for handoff to another AI"
            >
              {exporting ? "…" : "Send to Window 4"}
            </button>
            <button
              onClick={() => sendToWindow4(true)}
              disabled={exporting || !left || !right}
              className="px-4 py-2 border border-tm-border text-tm-muted rounded text-sm hover:border-tm-accent disabled:opacity-50"
              title="Same export, also written to ~/.tracemind/exports/"
            >
              + Save to disk
            </button>
          </div>

          {result && (
            <div className="mb-6 p-4 rounded-lg border border-tm-border bg-tm-surface">
              <div className="text-xs uppercase tracking-wider text-tm-muted mb-2">
                Materialized result
              </div>
              <ResultStats result={result} />
            </div>
          )}

          {exported && (
            <div className="p-4 rounded-lg border border-tm-accent/40 bg-tm-accent/5">
              <div className="text-xs uppercase tracking-wider text-tm-accent mb-2">
                Portable export ready
              </div>
              <div className="text-sm text-tm-text">
                <div>
                  <span className="text-tm-muted">Schema version:</span>{" "}
                  {exported.graph.schema_version}
                </div>
                <div>
                  <span className="text-tm-muted">Nodes / edges:</span>{" "}
                  {exported.graph.nodes.length} / {exported.graph.edges.length}
                </div>
                <div>
                  <span className="text-tm-muted">Approx tokens:</span>{" "}
                  {exported.approx_tokens.toLocaleString()}
                </div>
                {exported.path && (
                  <div className="font-mono text-xs mt-1 text-tm-muted break-all">
                    saved → {exported.path}
                  </div>
                )}
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

function ThreadPicker({
  label,
  threads,
  value,
  onChange,
}: {
  label: string;
  threads: ThreadDto[];
  value: string;
  onChange: (id: string) => void;
}) {
  return (
    <div>
      <div className="text-xs uppercase tracking-wider text-tm-muted mb-1">
        {label}
      </div>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
      >
        {threads.map((t) => (
          <option key={t.id} value={t.id}>
            {t.title || t.id.slice(0, 8)}
            {t.ended_at ? " (ended)" : ""}
          </option>
        ))}
      </select>
    </div>
  );
}

function OpPicker({
  value,
  onChange,
}: {
  value: SetOp;
  onChange: (op: SetOp) => void;
}) {
  return (
    <div>
      <div className="text-xs uppercase tracking-wider text-tm-muted mb-1">
        Op
      </div>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value as SetOp)}
        className="w-full px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
      >
        {OPS.map((o) => (
          <option key={o.id} value={o.id} title={o.hint}>
            {o.label}
          </option>
        ))}
      </select>
      <div className="text-xs text-tm-muted mt-1">
        {OPS.find((o) => o.id === value)?.hint}
      </div>
    </div>
  );
}

function ResultStats({ result }: { result: ThreadGraphDto }) {
  const rows: Array<[string, number]> = [
    ["Events", result.event_node_ids.length],
    ["Entities", result.entity_ids.length],
    ["Clusters", result.topic_clusters.length],
    ["Commits", result.commitment_ids.length],
    ["Captures", result.capture_signal_ids.length],
  ];
  return (
    <div className="grid grid-cols-5 gap-3 text-xs">
      {rows.map(([label, n]) => (
        <div key={label} className="px-3 py-2 rounded bg-tm-bg border border-tm-border">
          <div className="text-tm-muted uppercase tracking-wider text-[10px]">
            {label}
          </div>
          <div className="text-tm-text text-lg font-semibold mt-0.5">{n}</div>
        </div>
      ))}
    </div>
  );
}
