import { useEffect, useRef, useState, useCallback } from "react";
import { getGraph, deleteEntity, type GraphData, type GraphEdge } from "../api";

const TYPE_COLORS: Record<string, string> = {
  Person: "#60a5fa",
  Organization: "#a78bfa",
  Technology: "#34d399",
  Concept: "#94a3b8",
  File: "#fbbf24",
  Url: "#22d3ee",
  Project: "#f472b6",
  Decision: "#fb923c",
  Event: "#fb7185",
};

// Community palette — 12 distinct hues for Louvain clusters
const COMMUNITY_COLORS = [
  "#7c5cfc", "#34d399", "#f472b6", "#fbbf24", "#60a5fa",
  "#fb923c", "#a78bfa", "#22d3ee", "#fb7185", "#94a3b8",
  "#e879f9", "#4ade80",
];

interface SimNode {
  id: string;
  name: string;
  entity_type: string;
  confidence: number;
  community: number | null;
  x: number;
  y: number;
  vx: number;
  vy: number;
  radius: number;
  degree: number;
}

function typeColor(t: string): string {
  return TYPE_COLORS[t] || "#6b6b80";
}

function communityColor(c: number | null): string {
  if (c === null) return "#6b6b80";
  return COMMUNITY_COLORS[c % COMMUNITY_COLORS.length];
}

export default function GraphView() {
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [data, setData] = useState<GraphData | null>(null);
  const [error, setError] = useState("");
  const [hovered, setHovered] = useState<SimNode | null>(null);
  const [filter, setFilter] = useState<string>("all");
  const [showLabels, setShowLabels] = useState(true);
  const [colorMode, setColorMode] = useState<"type" | "community">("type");
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; node: SimNode } | null>(null);
  const nodesRef = useRef<SimNode[]>([]);
  const edgesRef = useRef<GraphEdge[]>([]);
  const animRef = useRef<number>(0);
  const dragRef = useRef<{ node: SimNode; offsetX: number; offsetY: number } | null>(null);
  const sizeRef = useRef({ w: 800, h: 600 });
  // Viewport transform — pan + zoom. Lives in a ref so the rAF redraw
  // sees fresh values without restarting the effect.
  const viewRef = useRef({ tx: 0, ty: 0, scale: 1 });
  const panRef = useRef<{ startX: number; startY: number; startTx: number; startTy: number } | null>(null);
  // Mirror `hovered` into a ref so the draw loop can read it without the
  // simulation effect listing it as a dep (which would tear down + restart
  // the physics on every mouse move — exactly the "bouncing around" symptom).
  const hoveredRef = useRef<SimNode | null>(null);
  useEffect(() => { hoveredRef.current = hovered; }, [hovered]);
  // Bump this state to force a manual redraw after view-only changes
  // (zoom, pan, fit). The simulation rAF picks it up via the closure.
  const [, setRedraw] = useState(0);
  // Whether auto-fit has run for the current data load. Re-armed each
  // time `data` changes (see effect below).
  const autoFitDoneRef = useRef(false);

  // 2026-05-11 — Compute a viewport (tx/ty/scale) that frames every node
  // with a small padding. Called once after the force layout settles, and
  // when the user presses "Fit".
  const fitToScreen = useCallback(() => {
    const ns = nodesRef.current;
    if (!ns.length) return;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const n of ns) {
      if (n.x - n.radius < minX) minX = n.x - n.radius;
      if (n.y - n.radius < minY) minY = n.y - n.radius;
      if (n.x + n.radius > maxX) maxX = n.x + n.radius;
      if (n.y + n.radius > maxY) maxY = n.y + n.radius;
    }
    const { w, h } = sizeRef.current;
    const bw = Math.max(1, maxX - minX);
    const bh = Math.max(1, maxY - minY);
    const pad = 40;
    const sx = (w - pad * 2) / bw;
    const sy = (h - pad * 2) / bh;
    const s = Math.max(0.25, Math.min(4, Math.min(sx, sy)));
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    viewRef.current = {
      tx: w / 2 - cx * s,
      ty: h / 2 - cy * s,
      scale: s,
    };
    setRedraw((r) => r + 1);
  }, []);

  useEffect(() => {
    getGraph()
      .then(setData)
      .catch((e) => setError(String(e)));
  }, []);

  // Responsive canvas sizing
  const updateCanvasSize = useCallback(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = container.getBoundingClientRect();
    const w = Math.floor(rect.width);
    const h = Math.floor(rect.height);

    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;

    const ctx = canvas.getContext("2d");
    if (ctx) ctx.scale(dpr, dpr);

    sizeRef.current = { w, h };
  }, []);

  useEffect(() => {
    updateCanvasSize();
    const observer = new ResizeObserver(updateCanvasSize);
    if (containerRef.current) observer.observe(containerRef.current);
    return () => observer.disconnect();
  }, [updateCanvasSize]);

  // Get unique types from data for filter dropdown
  const entityTypes = data
    ? Array.from(new Set(data.nodes.map((n) => n.entity_type))).sort()
    : [];

  useEffect(() => {
    if (!data || !canvasRef.current) return;

    const canvas = canvasRef.current;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    // Filter nodes
    const filteredNodes = filter === "all"
      ? data.nodes
      : data.nodes.filter((n) => n.entity_type === filter);
    const nodeIds = new Set(filteredNodes.map((n) => n.id));

    // Filter edges to only include edges between visible nodes
    const filteredEdges = data.edges.filter(
      (e) => nodeIds.has(e.source) && nodeIds.has(e.target)
    );

    // Compute node degrees for sizing
    const degreeMap = new Map<string, number>();
    for (const e of filteredEdges) {
      degreeMap.set(e.source, (degreeMap.get(e.source) || 0) + 1);
      degreeMap.set(e.target, (degreeMap.get(e.target) || 0) + 1);
    }

    const { w, h } = sizeRef.current;

    // Init nodes on a concentric circle layout — much faster to settle
    // than random scatter, and avoids the "explosion" of the first ~200
    // frames where everything was bouncing off the boundaries.
    const N = filteredNodes.length;
    const baseRadius = Math.min(w, h) * 0.32;
    const nodes: SimNode[] = filteredNodes.map((n, idx) => {
      const degree = degreeMap.get(n.id) || 0;
      // High-degree nodes start nearer the center; low-degree at the rim.
      // This is a cheap proxy for "communities first, leaves last".
      const tier = degree > 3 ? 0.35 : degree > 0 ? 0.75 : 1.0;
      const angle = (idx / Math.max(N, 1)) * Math.PI * 2;
      return {
        id: n.id,
        name: n.name,
        entity_type: n.entity_type,
        confidence: n.confidence,
        community: n.community,
        x: w / 2 + Math.cos(angle) * baseRadius * tier,
        y: h / 2 + Math.sin(angle) * baseRadius * tier,
        vx: 0,
        vy: 0,
        radius: Math.max(5, Math.min(22, 6 + Math.sqrt(degree) * 3 + n.confidence * 4)),
        degree,
      };
    });
    nodesRef.current = nodes;
    edgesRef.current = filteredEdges;
    // Re-arm auto-fit for the new layout.
    autoFitDoneRef.current = false;

    const idToNode = new Map<string, SimNode>();
    nodes.forEach((n) => idToNode.set(n.id, n));

    let frameCount = 0;
    const maxSimFrames = 300; // stop simulation after settling
    let settled = false;
    // Pin the canvas's logical CSS resolution. The `2d` context's
    // transform was already scaled to dpr in updateCanvasSize, so per-frame
    // resets must use the same factor or hi-dpi screens get fractional offsets.
    const dpr = window.devicePixelRatio || 1;

    const simulate = () => {
      const { w, h } = sizeRef.current;
      const nodeCount = nodes.length;
      const isLargeGraph = nodeCount > 100;

      // Adaptive parameters based on graph size
      const k = isLargeGraph ? 80 : 120;
      const gravity = isLargeGraph ? 0.015 : 0.01;
      const damping = 0.85;
      const repulsionScale = isLargeGraph ? 0.03 : 0.05;

      // Cooling: ramp down to 0 (not 0.2!) once we pass maxSimFrames so the
      // system can actually settle. The old 0.2 floor kept gravity injecting
      // force forever and produced the visible drift.
      const cooling = settled
        ? 0
        : frameCount < maxSimFrames
          ? 1.0 - (frameCount / maxSimFrames) * 0.9
          : Math.max(0, 0.1 - (frameCount - maxSimFrames) / 200);

      // Center gravity
      for (const n of nodes) {
        n.vx += (w / 2 - n.x) * gravity * cooling;
        n.vy += (h / 2 - n.y) * gravity * cooling;
      }

      // Repulsion (with spatial cutoff for large graphs)
      const cutoff = isLargeGraph ? k * 4 : Infinity;
      for (let i = 0; i < nodeCount; i++) {
        for (let j = i + 1; j < nodeCount; j++) {
          const dx = nodes[j].x - nodes[i].x;
          const dy = nodes[j].y - nodes[i].y;
          const distSq = dx * dx + dy * dy;
          if (distSq > cutoff * cutoff) continue;
          const dist = Math.max(Math.sqrt(distSq), 1);
          const force = (k * k) / dist;
          const fx = (dx / dist) * force * repulsionScale * cooling;
          const fy = (dy / dist) * force * repulsionScale * cooling;
          nodes[i].vx -= fx;
          nodes[i].vy -= fy;
          nodes[j].vx += fx;
          nodes[j].vy += fy;
        }
      }

      // Attraction along edges
      for (const edge of filteredEdges) {
        const src = idToNode.get(edge.source);
        const tgt = idToNode.get(edge.target);
        if (!src || !tgt) continue;
        const dx = tgt.x - src.x;
        const dy = tgt.y - src.y;
        const dist = Math.max(Math.sqrt(dx * dx + dy * dy), 1);
        const force = (dist - k) * 0.015 * cooling;
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;
        src.vx += fx;
        src.vy += fy;
        tgt.vx -= fx;
        tgt.vy -= fy;
      }

      // Apply velocity + accumulate kinetic energy. KE is the convergence
      // signal — when it falls below a small epsilon and we're past the
      // initial bursty period, freeze the simulation entirely.
      let ke = 0;
      for (const n of nodes) {
        if (dragRef.current?.node === n) continue;
        n.vx *= damping;
        n.vy *= damping;
        n.x += n.vx;
        n.y += n.vy;
        // Soft boundary: pull back gently instead of clamping so nodes
        // don't get stuck on the wall (which used to drive the bouncing).
        const margin = n.radius + 5;
        if (n.x < margin) { n.x = margin; n.vx = 0; }
        else if (n.x > w - margin) { n.x = w - margin; n.vx = 0; }
        if (n.y < margin) { n.y = margin; n.vy = 0; }
        else if (n.y > h - margin) { n.y = h - margin; n.vy = 0; }
        ke += n.vx * n.vx + n.vy * n.vy;
      }
      const meanKe = ke / Math.max(nodes.length, 1);

      frameCount++;
      // Settle when energy is small and we've simulated long enough, OR
      // when we've blown past the hard cap regardless. After this point
      // we still draw (so drags and hovers stay responsive) but we apply
      // zero force, which is what makes the graph stop drifting.
      if (!settled && (
        (frameCount > 60 && meanKe < 0.04) ||
        frameCount > maxSimFrames * 2
      )) {
        settled = true;
        // 2026-05-11 — Auto-fit once the layout settles. Without this the
        // default viewport (tx=0, ty=0, scale=1) can leave the graph
        // partially off-screen on first load, especially with many nodes.
        if (!autoFitDoneRef.current) {
          autoFitDoneRef.current = true;
          fitToScreen();
        }
      }

      // Draw — apply dpr scale + viewport pan/zoom in one go.
      const { tx, ty, scale } = viewRef.current;
      ctx.save();
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      ctx.translate(tx, ty);
      ctx.scale(scale, scale);

      // Edges
      for (const edge of filteredEdges) {
        const src = idToNode.get(edge.source);
        const tgt = idToNode.get(edge.target);
        if (!src || !tgt) continue;
        const isRelatedTo = edge.predicate === "RelatedTo";
        ctx.beginPath();
        ctx.moveTo(src.x, src.y);
        ctx.lineTo(tgt.x, tgt.y);
        ctx.strokeStyle = isRelatedTo
          ? "rgba(100,100,130,0.15)"
          : "rgba(124,92,252,0.5)";
        ctx.lineWidth = isRelatedTo ? 0.5 : 1.5;
        ctx.stroke();

        // Edge label for typed predicates (only when not too many nodes)
        if (!isRelatedTo && !isLargeGraph) {
          const mx = (src.x + tgt.x) / 2;
          const my = (src.y + tgt.y) / 2;
          ctx.font = "9px -apple-system, sans-serif";
          ctx.fillStyle = "rgba(124,92,252,0.7)";
          ctx.textAlign = "center";
          ctx.fillText(edge.predicate, mx, my - 4);
        }
      }

      // Nodes
      for (const n of nodes) {
        const color = colorMode === "community" ? communityColor(n.community) : typeColor(n.entity_type);
        const isHovered = hoveredRef.current?.id === n.id;

        // Glow for hovered
        if (isHovered) {
          ctx.beginPath();
          ctx.arc(n.x, n.y, n.radius + 4, 0, Math.PI * 2);
          ctx.fillStyle = color + "22";
          ctx.fill();
        }

        ctx.beginPath();
        ctx.arc(n.x, n.y, n.radius, 0, Math.PI * 2);
        ctx.fillStyle = color + "44";
        ctx.fill();
        ctx.strokeStyle = color;
        ctx.lineWidth = isHovered ? 2.5 : 1.5;
        ctx.stroke();

        // Label (skip for large graphs unless hovered or high-degree)
        if (showLabels && (!isLargeGraph || isHovered || n.degree > 3)) {
          ctx.font = `${isHovered ? "bold 12" : "11"}px -apple-system, sans-serif`;
          ctx.fillStyle = isHovered ? "#ffffff" : "#c0c0d0";
          ctx.textAlign = "center";
          const label = n.name.length > 20 ? n.name.slice(0, 18) + "…" : n.name;
          ctx.fillText(label, n.x, n.y + n.radius + 14);
        }
      }

      ctx.restore();

      animRef.current = requestAnimationFrame(simulate);
    };

    animRef.current = requestAnimationFrame(simulate);

    return () => cancelAnimationFrame(animRef.current);
    // NOTE: `hovered` is intentionally NOT a dep — see hoveredRef above.
    // Listing it would restart the simulation (and the "explosion" reset)
    // on every mouse move.
  }, [data, filter, showLabels, colorMode, fitToScreen]);

  // Mouse interaction
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    // Convert a screen-space mouse event into world-space (post-zoom/pan)
    // canvas coordinates. Every hit-test and drag uses this.
    const toWorld = (e: MouseEvent): { x: number; y: number } => {
      const rect = canvas.getBoundingClientRect();
      const scaleX = sizeRef.current.w / rect.width;
      const scaleY = sizeRef.current.h / rect.height;
      const cssX = (e.clientX - rect.left) * scaleX;
      const cssY = (e.clientY - rect.top) * scaleY;
      const { tx, ty, scale } = viewRef.current;
      return { x: (cssX - tx) / scale, y: (cssY - ty) / scale };
    };

    const getNode = (e: MouseEvent): SimNode | null => {
      const { x: mx, y: my } = toWorld(e);
      for (const n of nodesRef.current) {
        const dx = mx - n.x;
        const dy = my - n.y;
        if (dx * dx + dy * dy < (n.radius + 6) * (n.radius + 6)) return n;
      }
      return null;
    };

    const onMove = (e: MouseEvent) => {
      if (dragRef.current) {
        const { x, y } = toWorld(e);
        dragRef.current.node.x = x - dragRef.current.offsetX;
        dragRef.current.node.y = y - dragRef.current.offsetY;
        dragRef.current.node.vx = 0;
        dragRef.current.node.vy = 0;
      } else if (panRef.current) {
        // Two-finger or background drag: pan the viewport.
        const rect = canvas.getBoundingClientRect();
        const scaleX = sizeRef.current.w / rect.width;
        const scaleY = sizeRef.current.h / rect.height;
        const dx = (e.clientX - rect.left) * scaleX - panRef.current.startX;
        const dy = (e.clientY - rect.top) * scaleY - panRef.current.startY;
        viewRef.current.tx = panRef.current.startTx + dx;
        viewRef.current.ty = panRef.current.startTy + dy;
      } else {
        const node = getNode(e);
        setHovered(node);
        canvas.style.cursor = node ? "grab" : "move";
      }
    };

    const onDown = (e: MouseEvent) => {
      const node = getNode(e);
      if (node) {
        const { x, y } = toWorld(e);
        dragRef.current = {
          node,
          offsetX: x - node.x,
          offsetY: y - node.y,
        };
        canvas.style.cursor = "grabbing";
      } else {
        // Background click → start a viewport pan.
        const rect = canvas.getBoundingClientRect();
        const scaleX = sizeRef.current.w / rect.width;
        const scaleY = sizeRef.current.h / rect.height;
        panRef.current = {
          startX: (e.clientX - rect.left) * scaleX,
          startY: (e.clientY - rect.top) * scaleY,
          startTx: viewRef.current.tx,
          startTy: viewRef.current.ty,
        };
        canvas.style.cursor = "grabbing";
      }
    };

    const onUp = () => {
      dragRef.current = null;
      panRef.current = null;
      canvas.style.cursor = "default";
    };

    // Wheel zoom around the cursor position so the spot under the
    // pointer stays put as we zoom in/out. clamp scale to [0.25, 4].
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const scaleX = sizeRef.current.w / rect.width;
      const scaleY = sizeRef.current.h / rect.height;
      const cssX = (e.clientX - rect.left) * scaleX;
      const cssY = (e.clientY - rect.top) * scaleY;
      const v = viewRef.current;
      const oldScale = v.scale;
      const factor = Math.exp(-e.deltaY * 0.0015);
      const newScale = Math.max(0.25, Math.min(4, oldScale * factor));
      // Keep world point under cursor stationary: tx' = cssX - (cssX - tx) * (new/old)
      v.tx = cssX - (cssX - v.tx) * (newScale / oldScale);
      v.ty = cssY - (cssY - v.ty) * (newScale / oldScale);
      v.scale = newScale;
      setRedraw((r) => r + 1);
    };

    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      const node = getNode(e);
      if (node) {
        setContextMenu({ x: e.clientX, y: e.clientY, node });
      } else {
        setContextMenu(null);
      }
    };

    canvas.addEventListener("mousemove", onMove);
    canvas.addEventListener("mousedown", onDown);
    canvas.addEventListener("mouseup", onUp);
    canvas.addEventListener("mouseleave", onUp);
    canvas.addEventListener("contextmenu", onContextMenu);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      canvas.removeEventListener("mousemove", onMove);
      canvas.removeEventListener("mousedown", onDown);
      canvas.removeEventListener("mouseup", onUp);
      canvas.removeEventListener("mouseleave", onUp);
      canvas.removeEventListener("contextmenu", onContextMenu);
      canvas.removeEventListener("wheel", onWheel);
    };
  }, [data]);

  if (error) return <div className="text-tm-red p-4">Error: {error}</div>;

  return (
    <div className="space-y-3 h-full flex flex-col">
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-semibold">Knowledge Graph</h2>
        <div className="flex items-center gap-3">
          {data && (
            <span className="text-xs text-tm-muted">
              {nodesRef.current.length} nodes, {edgesRef.current.length} edges
              {filter !== "all" && ` (filtered from ${data.nodes.length})`}
            </span>
          )}
          <select
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="text-xs px-2 py-1 rounded bg-tm-surface border border-tm-border text-tm-text"
          >
            <option value="all">All Types</option>
            {entityTypes.map((t) => (
              <option key={t} value={t}>{t}</option>
            ))}
          </select>
          <button
            onClick={() => setColorMode(colorMode === "type" ? "community" : "type")}
            className={`text-xs px-3 py-1 rounded border transition-colors ${
              colorMode === "community"
                ? "bg-emerald-500/20 border-emerald-500/40 text-emerald-400"
                : "bg-tm-surface border-tm-border text-tm-muted"
            }`}
          >
            {colorMode === "community" ? "Communities" : "By Type"}
          </button>
          <button
            onClick={() => setShowLabels(!showLabels)}
            className={`text-xs px-3 py-1 rounded border transition-colors ${
              showLabels
                ? "bg-tm-accent/20 border-tm-accent/40 text-tm-accent"
                : "bg-tm-surface border-tm-border text-tm-muted"
            }`}
          >
            Labels
          </button>
          <button
            onClick={fitToScreen}
            className="text-xs px-3 py-1 rounded bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text transition-colors"
            title="Frame all nodes"
          >
            Fit
          </button>
          <button
            onClick={() => {
              getGraph().then(setData).catch((e) => setError(String(e)));
            }}
            className="text-xs px-3 py-1 rounded bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text transition-colors"
          >
            Refresh
          </button>
        </div>
      </div>

      {/* Legend */}
      <div className="flex flex-wrap gap-3">
        {Object.entries(TYPE_COLORS).map(([type, color]) => (
          <div key={type} className="flex items-center gap-1.5 text-xs">
            <div className="w-2.5 h-2.5 rounded-full" style={{ backgroundColor: color }} />
            <span className="text-tm-muted">{type}</span>
          </div>
        ))}
      </div>

      {/* Hover info */}
      {hovered && (
        <div className="bg-tm-surface border border-tm-border rounded-lg px-4 py-2 flex items-center gap-4 text-sm">
          <span
            className="w-3 h-3 rounded-full flex-shrink-0"
            style={{ backgroundColor: typeColor(hovered.entity_type) }}
          />
          <span className="font-medium">{hovered.name}</span>
          <span className="text-tm-muted text-xs">{hovered.entity_type}</span>
          <span className="text-tm-muted text-xs">confidence: {(hovered.confidence * 100).toFixed(0)}%</span>
          <span className="text-tm-muted text-xs">{hovered.degree} connections</span>
          {hovered.community !== null && (
            <span className="text-xs" style={{ color: communityColor(hovered.community) }}>
              community {hovered.community}
            </span>
          )}
          <span className="text-tm-muted text-xs ml-auto">Drag to reposition &middot; Right-click for options</span>
        </div>
      )}

      {data && data.community_count > 0 && colorMode === "community" && (
        <div className="flex flex-wrap gap-3">
          {Array.from({ length: Math.min(data.community_count, 12) }, (_, i) => (
            <div key={i} className="flex items-center gap-1.5 text-xs">
              <div className="w-2.5 h-2.5 rounded-full" style={{ backgroundColor: COMMUNITY_COLORS[i] }} />
              <span className="text-tm-muted">Community {i}</span>
            </div>
          ))}
        </div>
      )}

      <div ref={containerRef} className="bg-tm-surface border border-tm-border rounded-lg overflow-hidden flex-1 min-h-0 relative" style={{ minHeight: 400 }}>
        <canvas
          ref={canvasRef}
          className="w-full h-full"
          onClick={() => setContextMenu(null)}
        />
      </div>

      {contextMenu && (
        <div
          className="fixed bg-tm-surface border border-tm-border rounded-lg shadow-xl py-1 z-50"
          style={{ left: contextMenu.x, top: contextMenu.y }}
        >
          <div className="px-3 py-1.5 text-xs text-tm-muted border-b border-tm-border">
            {contextMenu.node.name}
          </div>
          <button
            className="w-full text-left px-3 py-1.5 text-xs text-red-400 hover:bg-red-500/10 transition-colors"
            onClick={async () => {
              await deleteEntity(contextMenu.node.id);
              setContextMenu(null);
              getGraph().then(setData).catch((e) => setError(String(e)));
            }}
          >
            Delete Entity
          </button>
        </div>
      )}
    </div>
  );
}
