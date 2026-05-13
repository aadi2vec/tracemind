// LM-3 + LM-4 — composite entity drawer.
//
// Renders header + backlinks + outgoing relations + tags for one
// entity, hydrated through `cmd_entity_drawer` in a single round-trip.
// Listens for the `entity-updated` event so a clipboard / window capture
// that touches the open entity refreshes the drawer in place without a
// page reload.

import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  getEntityDrawer,
  type EntityDrawerView,
  type EntityUpdatedEvent,
} from "../api";

function shortId(id: string): string {
  return id.slice(0, 8);
}

export default function EntityDrawer({
  entityId,
  onClose,
  onOpenEntity,
}: {
  /** Either a UUID or a name — the backend resolves both. */
  entityId: string;
  onClose: () => void;
  /** Optional click-through navigation: a backlink / relation row
   * calls this to re-target the drawer at another entity. */
  onOpenEntity?: (target: string) => void;
}) {
  const [view, setView] = useState<EntityDrawerView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(async () => {
    setRefreshing(true);
    try {
      const v = await getEntityDrawer(entityId, 50);
      setView(v);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshing(false);
    }
  }, [entityId]);

  useEffect(() => {
    load();
  }, [load]);

  // LM-4 — refresh in place when capture touches this entity. Listener
  // is scoped to the drawer's lifetime so it tears down on close.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    (async () => {
      unlisten = await listen<EntityUpdatedEvent>("entity-updated", (evt) => {
        const ids = evt.payload?.entity_ids ?? [];
        if (
          view &&
          ids.some(
            (id) =>
              id === view.header.entity_id ||
              id.toLowerCase() === entityId.toLowerCase(),
          )
        ) {
          load();
        }
      });
    })();
    return () => {
      if (unlisten) unlisten();
    };
  }, [view, entityId, load]);

  return (
    <div className="fixed inset-0 z-50 flex">
      <div
        onClick={onClose}
        className="flex-1 bg-black/40 backdrop-blur-sm"
        aria-hidden
      />
      <div className="w-full max-w-lg bg-tm-bg border-l border-tm-border p-5 overflow-y-auto">
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-sm font-semibold text-tm-text">
            Entity{refreshing ? " · refreshing…" : ""}
          </h3>
          <button
            onClick={onClose}
            className="text-tm-muted hover:text-tm-text text-sm"
            aria-label="close"
          >
            ×
          </button>
        </div>

        {error && (
          <p className="text-sm text-rose-400 mb-3">Failed: {error}</p>
        )}

        {!view && !error && (
          <p className="text-sm text-tm-muted">Loading…</p>
        )}

        {view && (
          <>
            <section className="mb-5">
              <div className="text-base font-semibold text-tm-text">
                {view.header.name}
              </div>
              <div className="text-xs text-tm-muted mt-1 flex flex-wrap gap-x-3 gap-y-1">
                <span>
                  type{" "}
                  <span className="text-tm-text">{view.header.entity_type}</span>
                </span>
                <span>
                  domain{" "}
                  <span className="text-tm-accent">
                    {view.header.ontological_domain}
                  </span>
                </span>
                <span>
                  confidence{" "}
                  <span className="font-mono text-tm-text">
                    {view.header.confidence.toFixed(2)}
                  </span>
                </span>
              </div>
              <div className="text-xs text-tm-muted mt-1">
                <span className="font-mono">{shortId(view.header.entity_id)}</span>
                {" · created "}
                {view.header.created_at.slice(0, 10)}
                {" · updated "}
                {view.header.updated_at.slice(0, 10)}
              </div>
              {view.tags.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {view.tags.map((t) => (
                    <span
                      key={t}
                      className="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-tm-accent/10 text-tm-accent border border-tm-accent/30"
                    >
                      {t}
                    </span>
                  ))}
                </div>
              )}
            </section>

            <section className="mb-5">
              <h4 className="text-xs uppercase tracking-wider text-tm-muted mb-2">
                relations ({view.relations.length})
              </h4>
              {view.relations.length === 0 ? (
                <p className="text-xs text-tm-muted">none</p>
              ) : (
                <ul className="space-y-1">
                  {view.relations.map((r) => (
                    <li
                      key={r.triple_id}
                      className="text-sm flex items-baseline gap-2 group"
                    >
                      <span className="text-tm-muted text-xs">
                        {r.predicate}
                      </span>
                      <button
                        onClick={() => onOpenEntity?.(r.target_id)}
                        className="text-tm-text hover:text-tm-accent transition text-left"
                      >
                        {r.target_name}
                      </button>
                      <span className="ml-auto text-xs font-mono text-tm-muted">
                        {r.confidence.toFixed(2)}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="text-xs uppercase tracking-wider text-tm-muted mb-2">
                backlinks ({view.backlinks.length})
              </h4>
              {view.backlinks.length === 0 ? (
                <p className="text-xs text-tm-muted">none</p>
              ) : (
                <ul className="space-y-1">
                  {view.backlinks.map((b) => (
                    <li
                      key={b.triple_id}
                      className="text-sm flex items-baseline gap-2"
                    >
                      <button
                        onClick={() => onOpenEntity?.(b.source_id)}
                        className="text-tm-text hover:text-tm-accent transition text-left"
                      >
                        {b.source_name}
                      </button>
                      <span className="text-tm-muted text-xs">
                        {b.predicate}
                      </span>
                      <span className="ml-auto text-xs font-mono text-tm-muted">
                        {b.confidence.toFixed(2)}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
}
