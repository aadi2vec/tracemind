// ONT-2 — Statistical ontology proposals (Settings inline panel).
//
// Renders a compact list of pending Object Type proposals emitted by
// `tm-reflect::propose_object_types`. The user accepts or rejects each
// inline — accepted proposals become real Object Types tagged
// `source = 'statistical'`. Rejected proposals are remembered so we
// won't badger.
//
// This is deliberately *not* Foundry-shaped: no schema editor, no
// graph visualization, no power-user knobs. It's a list of plain
// suggestion cards with two buttons. The full schema editor lives one
// level down in Settings → Schema (power users).

import { useCallback, useEffect, useState } from "react";
import {
  ontologyAcceptProposal,
  ontologyProposals,
  ontologyRejectProposal,
  type OntologyProposalDto,
} from "../api";

export default function OntologyProposals() {
  const [rows, setRows] = useState<OntologyProposalDto[]>([]);
  const [err, setErr] = useState<string>("");
  const [pendingId, setPendingId] = useState<string | null>(null);

  const refresh = useCallback(() => {
    ontologyProposals()
      .then((rs) => {
        setRows(rs);
        setErr("");
      })
      .catch((e) => setErr(String(e)));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const onAccept = async (id: string) => {
    setPendingId(id);
    try {
      await ontologyAcceptProposal(id);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setPendingId(null);
    }
  };

  const onReject = async (id: string) => {
    setPendingId(id);
    try {
      await ontologyRejectProposal(id);
      refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setPendingId(null);
    }
  };

  if (rows.length === 0 && !err) {
    return (
      <section className="bg-tm-surface border border-tm-border rounded-lg p-5 mb-3">
        <h3 className="text-lg font-medium text-tm-text">
          Ontology proposals
        </h3>
        <p className="text-xs text-tm-muted mt-1 max-w-xl">
          When recurring clusters in your memory look like a new kind of
          thing, TraceMind will suggest naming them here. Nothing pending
          right now.
        </p>
      </section>
    );
  }

  return (
    <section className="bg-tm-surface border border-tm-border rounded-lg p-5 mb-3">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-lg font-medium text-tm-text">
          Ontology proposals
        </h3>
        <span className="text-xs text-tm-muted">
          {rows.length} pending
        </span>
      </div>
      <p className="text-xs text-tm-muted mb-3 max-w-xl">
        Statistical suggestions from your memory. Accepting one creates a
        new Object Type tagged <code>statistical</code>. Rejecting it
        won't be re-suggested.
      </p>
      {err && (
        <p className="text-xs text-red-500 mb-2" role="alert">
          {err}
        </p>
      )}
      <ul className="flex flex-col gap-2">
        {rows.map((p) => (
          <li
            key={p.id}
            className="flex items-start justify-between gap-3 border border-tm-border rounded-md px-3 py-2"
          >
            <div className="min-w-0">
              <div className="flex items-baseline gap-2">
                <span className="text-sm font-medium text-tm-text">
                  {p.name}
                </span>
                <span className="text-[10px] uppercase tracking-wide text-tm-muted">
                  {p.kind}
                </span>
                <span className="text-xs text-tm-muted">
                  · {p.support_count} obs
                </span>
              </div>
              {p.top_terms.length > 0 && (
                <p className="text-xs text-tm-muted mt-0.5 truncate">
                  {p.top_terms.slice(0, 5).join(" · ")}
                </p>
              )}
            </div>
            <div className="flex gap-2 shrink-0">
              <button
                onClick={() => onAccept(p.id)}
                disabled={pendingId === p.id}
                className="text-xs px-2 py-1 rounded bg-tm-accent text-tm-bg hover:opacity-90 disabled:opacity-40"
              >
                Accept
              </button>
              <button
                onClick={() => onReject(p.id)}
                disabled={pendingId === p.id}
                className="text-xs px-2 py-1 rounded border border-tm-border text-tm-muted hover:text-tm-text disabled:opacity-40"
              >
                Reject
              </button>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
