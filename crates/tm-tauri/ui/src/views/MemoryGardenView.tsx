// LM-20 / LM-22 / LM-23 — Memory Garden.
//
// Three stacked panels on one page:
//   1. Cluster cards (`cmd_memory_garden`)
//   2. Outlier tray ("Unsorted") with per-row triage (`cmd_outliers_list`,
//      `cmd_outlier_triage`)
//   3. Community overlay (`cmd_community_overlay`)
//
// Until HDBSCAN/Louvain are populated by the future `tm-cluster` pass,
// the cards collapse to a single "unsorted" bucket. Wiring is final
// and the API contracts are stable — the UI lights up as soon as
// `cluster_id` / `community_id` are filled in.

import { useEffect, useState } from "react";
import {
  getCommunityOverlay,
  getMemoryGarden,
  listOutliers,
  triageOutlier,
  type CommunityRow,
  type GardenCard,
  type OutlierAction,
  type OutlierRow,
} from "../api";

export default function MemoryGardenView() {
  const [cards, setCards] = useState<GardenCard[]>([]);
  const [outliers, setOutliers] = useState<OutlierRow[]>([]);
  const [communities, setCommunities] = useState<CommunityRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      const [c, o, co] = await Promise.all([
        getMemoryGarden(),
        listOutliers(50),
        getCommunityOverlay(),
      ]);
      setCards(c);
      setOutliers(o);
      setCommunities(co);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const triage = async (
    signalId: number,
    action: OutlierAction,
    targetCluster?: number,
  ) => {
    try {
      await triageOutlier(signalId, action, targetCluster);
      // Optimistic: drop the row locally so the user sees instant feedback,
      // then refresh in background so cluster card counts update.
      setOutliers((rs) => rs.filter((r) => r.signal_id !== signalId));
      refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  // Cluster ids we can target via "add to existing" — skip unsorted/ignore buckets.
  const targetableClusters = cards
    .map((c) => c.cluster_id)
    .filter((id): id is number => typeof id === "number" && id >= 0);

  return (
    <div className="max-w-4xl">
      <div className="flex items-baseline justify-between mb-5">
        <div>
          <h2 className="text-lg font-semibold text-tm-text">Memory Garden</h2>
          <p className="text-xs text-tm-muted mt-0.5">
            Browse by cluster · triage outliers · toggle communities
          </p>
        </div>
        <button
          onClick={refresh}
          disabled={loading}
          className="px-3 py-1.5 text-xs bg-tm-accent/10 text-tm-accent border border-tm-accent/30 rounded hover:bg-tm-accent/20"
        >
          {loading ? "refreshing…" : "refresh"}
        </button>
      </div>

      {error && (
        <p className="text-sm text-rose-400 mb-4">{error}</p>
      )}

      {/* --- Cluster cards ------------------------------------------------- */}
      <section className="mb-8">
        <h3 className="text-sm font-semibold text-tm-text mb-2">
          clusters ({cards.length})
        </h3>
        {cards.length === 0 ? (
          <p className="text-xs text-tm-muted">
            No clusters yet. Ingest more captures (clipboard or `tracemind
            ingest`) and they'll bucket here once <code>tm-cluster</code> runs.
          </p>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {cards.map((card) => (
              <div
                key={`${card.cluster_id ?? "unsorted"}`}
                className="border border-tm-border rounded p-3 bg-tm-surface/30"
              >
                <div className="flex items-baseline justify-between mb-2">
                  <div>
                    <span className="text-sm font-medium text-tm-text">
                      {card.label}
                    </span>
                    {card.cluster_id !== null && card.cluster_id >= 0 && (
                      <span className="text-xs text-tm-muted ml-2 font-mono">
                        #{card.cluster_id}
                      </span>
                    )}
                  </div>
                  <span className="text-xs font-mono text-tm-muted">
                    {card.count}
                  </span>
                </div>
                {card.sample_texts.length === 0 ? (
                  <p className="text-xs text-tm-muted">no samples</p>
                ) : (
                  <ul className="space-y-1">
                    {card.sample_texts.map((s, i) => (
                      <li
                        key={i}
                        className="text-xs text-tm-muted truncate"
                        title={s}
                      >
                        · {s}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      {/* --- Outlier tray ------------------------------------------------- */}
      <section className="mb-8">
        <h3 className="text-sm font-semibold text-tm-text mb-2">
          unsorted ({outliers.length})
        </h3>
        {outliers.length === 0 ? (
          <p className="text-xs text-tm-muted">No outliers — every capture is in a cluster.</p>
        ) : (
          <ul className="space-y-2">
            {outliers.map((row) => (
              <OutlierItem
                key={row.signal_id}
                row={row}
                targetableClusters={targetableClusters}
                onTriage={triage}
              />
            ))}
          </ul>
        )}
      </section>

      {/* --- Community overlay -------------------------------------------- */}
      <section>
        <h3 className="text-sm font-semibold text-tm-text mb-2">
          communities ({communities.length})
        </h3>
        {communities.length === 0 ? (
          <p className="text-xs text-tm-muted">No community data yet.</p>
        ) : (
          <ul className="space-y-1">
            {communities.map((c, i) => (
              <li
                key={`${c.community_id ?? "unassigned"}-${i}`}
                className="text-sm border-b border-tm-border/40 py-2 flex items-baseline gap-3"
              >
                <span className="text-xs font-mono text-tm-muted w-16 shrink-0">
                  {c.community_id === null ? "—" : `c${c.community_id}`}
                </span>
                <span className="text-tm-text">{c.entity_count} entities</span>
                <span className="text-xs text-tm-muted truncate flex-1">
                  {c.sample_names.slice(0, 5).join(" · ")}
                </span>
                {c.community_id !== null && (
                  // LM-22 — drill-down: open the graph view focused on this
                  // community. App.tsx listens for `tm:next-action` with
                  // target_kind=entity to route to the Graph view, and
                  // GraphView listens for `tm:focus-community` to apply the
                  // filter. Dispatching both keeps this view independent.
                  <button
                    onClick={() => {
                      window.dispatchEvent(
                        new CustomEvent("tm:next-action", {
                          detail: { target_kind: "entity" },
                        }),
                      );
                      // Defer the focus event one tick so GraphView has
                      // mounted and registered its listener.
                      setTimeout(() => {
                        window.dispatchEvent(
                          new CustomEvent("tm:focus-community", {
                            detail: { community_id: c.community_id },
                          }),
                        );
                      }, 50);
                    }}
                    className="text-xs px-2 py-0.5 rounded border border-emerald-500/40 bg-emerald-500/10 text-emerald-300 hover:bg-emerald-500/20 transition-colors shrink-0"
                    title="Open in Graph view, focused on this community"
                  >
                    view in graph →
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function OutlierItem({
  row,
  targetableClusters,
  onTriage,
}: {
  row: OutlierRow;
  targetableClusters: number[];
  onTriage: (
    signalId: number,
    action: OutlierAction,
    targetCluster?: number,
  ) => void;
}) {
  const [picking, setPicking] = useState(false);
  return (
    <li className="border border-tm-border rounded p-3">
      <div className="flex items-baseline gap-2 mb-1.5">
        <span className="text-xs text-tm-muted">{row.source}</span>
        <span className="text-xs text-tm-muted">
          {row.created_at.slice(0, 19).replace("T", " ")}
        </span>
      </div>
      <p className="text-sm text-tm-text mb-2 break-words">
        {row.raw_text.length > 240
          ? `${row.raw_text.slice(0, 240)}…`
          : row.raw_text}
      </p>
      <div className="flex items-center gap-2">
        <button
          onClick={() => onTriage(row.signal_id, "new_cluster")}
          className="px-2 py-1 text-xs border border-emerald-500/40 bg-emerald-500/10 text-emerald-300 rounded hover:bg-emerald-500/20"
        >
          new cluster
        </button>
        <button
          onClick={() => setPicking((v) => !v)}
          disabled={targetableClusters.length === 0}
          className="px-2 py-1 text-xs border border-tm-accent/40 bg-tm-accent/10 text-tm-accent rounded hover:bg-tm-accent/20 disabled:opacity-40"
        >
          add to…
        </button>
        <button
          onClick={() => onTriage(row.signal_id, "ignore")}
          className="px-2 py-1 text-xs border border-rose-500/40 bg-rose-500/10 text-rose-300 rounded hover:bg-rose-500/20"
        >
          ignore
        </button>
      </div>
      {picking && targetableClusters.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1">
          {targetableClusters.map((cid) => (
            <button
              key={cid}
              onClick={() => {
                setPicking(false);
                onTriage(row.signal_id, "add_to", cid);
              }}
              className="px-2 py-0.5 text-xs font-mono border border-tm-border text-tm-text rounded hover:bg-tm-accent/10"
            >
              #{cid}
            </button>
          ))}
        </div>
      )}
    </li>
  );
}
