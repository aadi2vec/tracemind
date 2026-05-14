// Sprint GRAPH — Ontology surface.
//
// Palantir-style ontology editor. Object Types name the kinds of things
// you remember (Person, Project, Decision, …). Link Types declare which
// edges are allowed (Project → Decision via "made_decision"). Together
// they give your memory a versioned, queryable schema. Entities can be
// retyped by name for quick assignment.
import { useEffect, useState } from "react";
import {
  ontologyAssign,
  ontologyCreateLinkType,
  ontologyCreateObjectType,
  ontologyList,
  type OntologyDto,
} from "../api";

export default function OntologyView() {
  const [ont, setOnt] = useState<OntologyDto | null>(null);
  const [err, setErr] = useState<string>("");
  const [info, setInfo] = useState<string>("");

  const [newObjType, setNewObjType] = useState("");
  const [newObjParent, setNewObjParent] = useState("");

  const [newLinkName, setNewLinkName] = useState("");
  const [newLinkFrom, setNewLinkFrom] = useState("");
  const [newLinkTo, setNewLinkTo] = useState("");

  const [assignEntityId, setAssignEntityId] = useState("");
  const [assignType, setAssignType] = useState("");

  const refresh = () => {
    ontologyList()
      .then((o) => {
        setOnt(o);
        setErr("");
      })
      .catch((e) => setErr(String(e)));
  };

  useEffect(() => {
    refresh();
  }, []);

  useEffect(() => {
    if (ont && ont.object_types.length > 0) {
      if (!newLinkFrom) setNewLinkFrom(ont.object_types[0]);
      if (!newLinkTo)
        setNewLinkTo(ont.object_types[Math.min(1, ont.object_types.length - 1)]);
      if (!assignType) setAssignType(ont.object_types[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ont]);

  const createObj = async () => {
    if (!newObjType.trim()) return;
    try {
      await ontologyCreateObjectType(
        newObjType.trim(),
        newObjParent.trim() || undefined,
      );
      setInfo(`Created object type "${newObjType.trim()}"`);
      setNewObjType("");
      setNewObjParent("");
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  const createLink = async () => {
    if (!newLinkName.trim() || !newLinkFrom || !newLinkTo) return;
    try {
      await ontologyCreateLinkType(newLinkName.trim(), newLinkFrom, newLinkTo);
      setInfo(
        `Created link "${newLinkName.trim()}" (${newLinkFrom} → ${newLinkTo})`,
      );
      setNewLinkName("");
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  const assign = async () => {
    if (!assignEntityId.trim() || !assignType) return;
    try {
      await ontologyAssign(assignEntityId.trim(), assignType);
      setInfo(
        `Assigned ${assignEntityId.trim().slice(0, 8)} → ${assignType}`,
      );
      setAssignEntityId("");
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="max-w-5xl">
      <div className="mb-6">
        <h2 className="text-2xl font-semibold text-tm-text">Ontology</h2>
        <p className="text-sm text-tm-muted mt-1">
          Object Types and Link Types are TraceMind's versioned schema —
          edits here update the type-check rules every new edge passes.
        </p>
      </div>

      {err && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-red/10 border border-tm-red/30 text-tm-red text-sm">
          {err}
        </div>
      )}
      {info && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-accent/10 border border-tm-accent/30 text-tm-accent text-sm">
          {info}
        </div>
      )}

      {ont === null ? (
        <div className="text-sm text-tm-muted">Loading…</div>
      ) : (
        <div className="grid grid-cols-2 gap-4">
          <Panel title={`Object Types (${ont.object_types.length})`}>
            <div className="max-h-72 overflow-y-auto space-y-1 mb-3">
              {ont.object_types.map((t) => (
                <div
                  key={t}
                  className="px-2 py-1 rounded text-sm font-mono text-tm-text bg-tm-bg border border-tm-border"
                >
                  {t}
                </div>
              ))}
            </div>
            <div className="border-t border-tm-border pt-3">
              <div className="text-xs uppercase tracking-wider text-tm-muted mb-2">
                Add object type
              </div>
              <input
                value={newObjType}
                onChange={(e) => setNewObjType(e.target.value)}
                placeholder="e.g. Habit"
                className="w-full mb-2 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent"
              />
              <input
                value={newObjParent}
                onChange={(e) => setNewObjParent(e.target.value)}
                placeholder="Parent type (optional)"
                className="w-full mb-2 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent"
              />
              <button
                onClick={createObj}
                disabled={!newObjType.trim()}
                className="w-full px-3 py-2 bg-tm-accent text-black rounded text-sm font-medium hover:bg-tm-accent/90 disabled:opacity-50"
              >
                Create
              </button>
            </div>
          </Panel>

          <Panel title={`Link Types (${ont.link_types.length})`}>
            <div className="max-h-72 overflow-y-auto space-y-1 mb-3">
              {ont.link_types.map((l) => (
                <div
                  key={`${l.name}|${l.from}|${l.to}`}
                  className="px-2 py-1 rounded text-xs font-mono text-tm-text bg-tm-bg border border-tm-border"
                >
                  <span className="text-tm-accent">{l.name}</span>{" "}
                  <span className="text-tm-muted">
                    ({l.from} → {l.to})
                  </span>
                </div>
              ))}
            </div>
            <div className="border-t border-tm-border pt-3">
              <div className="text-xs uppercase tracking-wider text-tm-muted mb-2">
                Add link type
              </div>
              <input
                value={newLinkName}
                onChange={(e) => setNewLinkName(e.target.value)}
                placeholder="e.g. supports"
                className="w-full mb-2 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent"
              />
              <div className="grid grid-cols-2 gap-2 mb-2">
                <select
                  value={newLinkFrom}
                  onChange={(e) => setNewLinkFrom(e.target.value)}
                  className="px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
                >
                  {ont.object_types.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
                <select
                  value={newLinkTo}
                  onChange={(e) => setNewLinkTo(e.target.value)}
                  className="px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
                >
                  {ont.object_types.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </div>
              <button
                onClick={createLink}
                disabled={!newLinkName.trim()}
                className="w-full px-3 py-2 bg-tm-accent text-black rounded text-sm font-medium hover:bg-tm-accent/90 disabled:opacity-50"
              >
                Create
              </button>
            </div>
          </Panel>

          <Panel title="Assign entity → object type">
            <div className="text-xs text-tm-muted mb-2">
              Type the entity ID (from Graph view) and pick its type.
            </div>
            <input
              value={assignEntityId}
              onChange={(e) => setAssignEntityId(e.target.value)}
              placeholder="entity id"
              className="w-full mb-2 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text placeholder-tm-muted focus:outline-none focus:border-tm-accent font-mono"
            />
            <select
              value={assignType}
              onChange={(e) => setAssignType(e.target.value)}
              className="w-full mb-2 px-3 py-2 bg-tm-bg border border-tm-border rounded text-sm text-tm-text focus:outline-none focus:border-tm-accent"
            >
              {ont.object_types.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
            <button
              onClick={assign}
              disabled={!assignEntityId.trim()}
              className="w-full px-3 py-2 bg-tm-accent text-black rounded text-sm font-medium hover:bg-tm-accent/90 disabled:opacity-50"
            >
              Assign
            </button>
          </Panel>
        </div>
      )}
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="p-4 rounded-lg border border-tm-border bg-tm-surface">
      <div className="text-sm font-medium text-tm-text mb-3">{title}</div>
      {children}
    </div>
  );
}
