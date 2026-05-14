// Sprint GRAPH — Event graph surface.
//
// Glean-style action graph: timestamped nodes (captures, queries,
// commitments, outcomes, decisions) connected by typed edges. Edges only
// become visible once their support count clears the frequency floor
// (MIN_SUPPORT = 2). This view renders a lightweight force layout — no
// d3 dep, just SVG and React state — so users can see the action graph
// that drives anticipatory predictions.
import { useEffect, useMemo, useRef, useState } from "react";
import {
  eventGraphFetch,
  eventPromote,
  type EventEdgeDto,
  type EventGraphDto,
  type EventNodeDto,
} from "../api";

const KIND_COLOR: Record<string, string> = {
  capture: "#7dd3fc",
  query: "#fcd34d",
  commitment: "#a78bfa",
  outcome: "#86efac",
  decision: "#f472b6",
};

const EDGE_COLOR: Record<string, string> = {
  precedes: "rgba(255,255,255,0.20)",
  caused: "rgba(244,114,182,0.50)",
  co_occurs: "rgba(125,211,252,0.35)",
  resolves: "rgba(134,239,172,0.55)",
  contradicts: "rgba(248,113,113,0.65)",
};

export default function EventGraphView() {
  const [graph, setGraph] = useState<EventGraphDto | null>(null);
  const [err, setErr] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [promoting, setPromoting] = useState(false);
  const [promoted, setPromoted] = useState<number | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);

  const refresh = () => {
    setLoading(true);
    eventGraphFetch(500)
      .then((g) => {
        setGraph(g);
        setErr("");
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    refresh();
  }, []);

  const promote = async () => {
    setPromoting(true);
    try {
      const n = await eventPromote();
      setPromoted(n);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setPromoting(false);
    }
  };

  return (
    <div className="max-w-6xl">
      <div className="mb-6 flex items-end justify-between gap-3">
        <div>
          <h2 className="text-2xl font-semibold text-tm-text">Event Graph</h2>
          <p className="text-sm text-tm-muted mt-1">
            Glean-style action graph. Edges are only shown once their support
            count clears the frequency floor — preventing tangle.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={refresh}
            disabled={loading}
            className="px-3 py-2 border border-tm-border rounded text-sm text-tm-text hover:border-tm-accent disabled:opacity-50"
          >
            {loading ? "…" : "Refresh"}
          </button>
          <button
            onClick={promote}
            disabled={promoting}
            className="px-3 py-2 border border-tm-border rounded text-sm text-tm-text hover:border-tm-accent disabled:opacity-50"
            title="Scan recent events and promote cluster-to-cluster sequence edges"
          >
            {promoting ? "Promoting…" : "Promote sequences"}
          </button>
        </div>
      </div>

      {err && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-red/10 border border-tm-red/30 text-tm-red text-sm">
          {err}
        </div>
      )}
      {promoted !== null && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-accent/10 border border-tm-accent/30 text-tm-accent text-sm">
          Promoted {promoted} edge{promoted === 1 ? "" : "s"} above the
          frequency floor.
        </div>
      )}

      <Legend />

      {graph === null ? (
        <div className="text-sm text-tm-muted">Loading…</div>
      ) : graph.nodes.length === 0 ? (
        <div className="text-sm text-tm-muted px-4 py-8 rounded border border-dashed border-tm-border text-center">
          No events yet. They populate automatically as you ingest captures,
          run queries, and commit to next actions.
        </div>
      ) : (
        <>
          <ForceLayout
            nodes={graph.nodes}
            edges={graph.edges}
            hovered={hovered}
            onHover={setHovered}
          />
          {hovered && (
            <NodeDetail
              node={graph.nodes.find((n) => n.id === hovered) ?? null}
            />
          )}
        </>
      )}
    </div>
  );
}

function Legend() {
  return (
    <div className="flex flex-wrap gap-3 mb-3 text-xs text-tm-muted">
      {Object.entries(KIND_COLOR).map(([kind, color]) => (
        <div key={kind} className="flex items-center gap-1.5">
          <span
            className="inline-block w-3 h-3 rounded-full"
            style={{ background: color }}
          />
          {kind}
        </div>
      ))}
    </div>
  );
}

function NodeDetail({ node }: { node: EventNodeDto | null }) {
  if (!node) return null;
  return (
    <div className="mt-3 p-3 rounded border border-tm-border bg-tm-surface text-xs">
      <div className="text-tm-muted uppercase tracking-wider text-[10px]">
        {node.kind} · {new Date(node.ts * 1000).toLocaleString()}
      </div>
      <div className="text-tm-text font-mono mt-1">{node.id}</div>
      <div className="text-tm-muted mt-1">
        payload_ref: <span className="text-tm-text">{node.payload_ref}</span>
      </div>
      <div className="text-tm-muted">
        cluster: {node.cluster_id ?? "—"} · salience{" "}
        {node.salience.toFixed(2)}
      </div>
    </div>
  );
}

// --- minimal force layout (no external d3 dep) ---
//
// We seed positions with a deterministic spiral, then run ~80 iterations
// of repulsion + spring attraction. The graph is bounded into a 720x440
// viewport. This is intentionally simple — for >500 nodes we'd want a
// proper simulation, but the event graph stays sparse by construction
// thanks to the frequency floor.
const W = 760;
const H = 440;

type LaidOutNode = EventNodeDto & { x: number; y: number };

function ForceLayout({
  nodes,
  edges,
  hovered,
  onHover,
}: {
  nodes: EventNodeDto[];
  edges: EventEdgeDto[];
  hovered: string | null;
  onHover: (id: string | null) => void;
}) {
  const positions = useMemo(() => layout(nodes, edges), [nodes, edges]);
  const svgRef = useRef<SVGSVGElement>(null);

  return (
    <svg
      ref={svgRef}
      viewBox={`0 0 ${W} ${H}`}
      className="w-full rounded border border-tm-border bg-tm-surface"
      style={{ aspectRatio: `${W} / ${H}` }}
    >
      {edges.map((e, i) => {
        const from = positions.find((n) => n.id === e.from);
        const to = positions.find((n) => n.id === e.to);
        if (!from || !to) return null;
        const color = EDGE_COLOR[e.kind] ?? "rgba(255,255,255,0.15)";
        const w = Math.min(1 + Math.log2(e.support_count + 1) * 0.7, 4);
        return (
          <line
            key={i}
            x1={from.x}
            y1={from.y}
            x2={to.x}
            y2={to.y}
            stroke={color}
            strokeWidth={w}
          />
        );
      })}
      {positions.map((n) => {
        const isHovered = hovered === n.id;
        return (
          <g
            key={n.id}
            transform={`translate(${n.x},${n.y})`}
            onMouseEnter={() => onHover(n.id)}
            onMouseLeave={() => onHover(null)}
            style={{ cursor: "pointer" }}
          >
            <circle
              r={isHovered ? 7 : 5}
              fill={KIND_COLOR[n.kind] ?? "#94a3b8"}
              stroke={isHovered ? "#fff" : "rgba(0,0,0,0.3)"}
              strokeWidth={isHovered ? 1.5 : 1}
            />
          </g>
        );
      })}
    </svg>
  );
}

function layout(nodes: EventNodeDto[], edges: EventEdgeDto[]): LaidOutNode[] {
  const N = nodes.length;
  if (N === 0) return [];
  // Deterministic seed: golden-angle spiral about the center.
  const cx = W / 2;
  const cy = H / 2;
  const goldenAngle = Math.PI * (3 - Math.sqrt(5));
  const positions: LaidOutNode[] = nodes.map((n, i) => {
    const radius = Math.sqrt(i + 1) * 14;
    const theta = i * goldenAngle;
    return {
      ...n,
      x: cx + radius * Math.cos(theta),
      y: cy + radius * Math.sin(theta),
    };
  });

  const idx = new Map(positions.map((p, i) => [p.id, i]));
  const adj: Array<[number, number, number]> = [];
  for (const e of edges) {
    const a = idx.get(e.from);
    const b = idx.get(e.to);
    if (a !== undefined && b !== undefined) {
      adj.push([a, b, Math.max(0.4, Math.min(1.5, e.strength))]);
    }
  }

  const iters = 80;
  const k = 28; // ideal edge length
  const repulsion = 220;
  for (let t = 0; t < iters; t++) {
    const cooling = 1 - t / iters;
    const fx = new Array(N).fill(0);
    const fy = new Array(N).fill(0);

    // repulsion (O(N^2) — fine for sparse event graph)
    for (let i = 0; i < N; i++) {
      for (let j = i + 1; j < N; j++) {
        const dx = positions[i].x - positions[j].x;
        const dy = positions[i].y - positions[j].y;
        const d2 = dx * dx + dy * dy + 0.01;
        const f = repulsion / d2;
        const dxn = dx / Math.sqrt(d2);
        const dyn = dy / Math.sqrt(d2);
        fx[i] += dxn * f;
        fy[i] += dyn * f;
        fx[j] -= dxn * f;
        fy[j] -= dyn * f;
      }
    }

    // spring attraction
    for (const [a, b, s] of adj) {
      const dx = positions[a].x - positions[b].x;
      const dy = positions[a].y - positions[b].y;
      const d = Math.sqrt(dx * dx + dy * dy) + 0.01;
      const f = ((d - k) / d) * 0.05 * s;
      fx[a] -= dx * f;
      fy[a] -= dy * f;
      fx[b] += dx * f;
      fy[b] += dy * f;
    }

    // apply with cooling + bounds
    for (let i = 0; i < N; i++) {
      const step = 2.5 * cooling;
      positions[i].x = clamp(positions[i].x + fx[i] * step, 16, W - 16);
      positions[i].y = clamp(positions[i].y + fy[i] * step, 16, H - 16);
    }
  }

  return positions;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
