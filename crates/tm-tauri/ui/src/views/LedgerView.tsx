// CTX-EVG-C — Commitment Ledger surface.
//
// The closing arrow on the system-of-intents wedge. Shows this week's
// kept / broken / pending score and lets the user resolve open
// commitments inline. Reads `cmd_commitment_ledger`; writes via
// `cmd_commitment_resolve` and `cmd_commitment_set_state`.
//
// Design rule from `tracemind_outcome_bond_stickiness.md`: friction in
// the resolution flow kills the loop. Resolve = single tap. Mark
// abandoned = single tap. Never modal.

import { useEffect, useMemo, useState } from "react";
import {
  commitmentLedger,
  commitmentResolve,
  commitmentSetState,
  type CommitmentDto,
  type CommitmentStateStr,
  type LedgerDto,
} from "../api";

const WINDOWS: Array<{ label: string; days: number }> = [
  { label: "7d", days: 7 },
  { label: "30d", days: 30 },
  { label: "all", days: 0 },
];

export default function LedgerView() {
  const [windowDays, setWindowDays] = useState<number>(7);
  const [data, setData] = useState<LedgerDto | null>(null);
  const [err, setErr] = useState<string>("");
  const [pendingId, setPendingId] = useState<string | null>(null);

  const refresh = (days: number) => {
    commitmentLedger(days)
      .then((d) => {
        setData(d);
        setErr("");
      })
      .catch((e) => setErr(String(e)));
  };

  useEffect(() => {
    refresh(windowDays);
  }, [windowDays]);

  const score = useMemo(() => {
    if (!data) return null;
    const total = data.kept + data.broken;
    const rate = total === 0 ? null : Math.round((data.kept / total) * 100);
    return { rate, total };
  }, [data]);

  const resolve = async (cid: string, polarity: number) => {
    setPendingId(cid);
    try {
      await commitmentResolve(cid, polarity, "Resolved from Ledger");
      refresh(windowDays);
    } catch (e) {
      setErr(String(e));
    } finally {
      setPendingId(null);
    }
  };

  const markState = async (cid: string, state: Exclude<CommitmentStateStr, "">) => {
    setPendingId(cid);
    try {
      await commitmentSetState(cid, state);
      refresh(windowDays);
    } catch (e) {
      setErr(String(e));
    } finally {
      setPendingId(null);
    }
  };

  return (
    <div className="max-w-5xl">
      <div className="mb-6 flex items-end justify-between">
        <div>
          <h2 className="text-2xl font-semibold text-tm-text">Commitment Ledger</h2>
          <p className="text-sm text-tm-muted mt-1">
            Did you do what you said you would? The closing arrow on every
            captured intent — kept, broken, pending. Resolve one in a tap.
          </p>
        </div>
        <div className="flex items-center gap-1 rounded border border-tm-border bg-tm-surface p-1">
          {WINDOWS.map((w) => (
            <button
              key={w.days}
              onClick={() => setWindowDays(w.days)}
              className={`px-3 py-1 text-xs rounded ${
                windowDays === w.days
                  ? "bg-tm-accent text-black font-medium"
                  : "text-tm-muted hover:text-tm-text"
              }`}
            >
              {w.label}
            </button>
          ))}
        </div>
      </div>

      {err && (
        <div className="mb-4 px-4 py-2 rounded bg-tm-red/10 border border-tm-red/30 text-tm-red text-sm">
          {err}
        </div>
      )}

      {data === null ? (
        <div className="text-sm text-tm-muted">Loading…</div>
      ) : (
        <>
          <ScoreCard data={data} rate={score?.rate ?? null} />
          <CommitmentsTable
            commitments={data.commitments}
            pendingId={pendingId}
            onResolve={resolve}
            onSetState={markState}
          />
        </>
      )}
    </div>
  );
}

function ScoreCard({
  data,
  rate,
}: {
  data: LedgerDto;
  rate: number | null;
}) {
  const stats: Array<[string, number, string]> = [
    ["kept", data.kept, "text-tm-green"],
    ["broken", data.broken, "text-tm-red"],
    ["pending", data.pending, "text-tm-accent"],
    ["abandoned", data.abandoned, "text-tm-muted"],
  ];
  return (
    <div className="mb-6 p-5 rounded-lg border border-tm-border bg-tm-surface">
      <div className="grid grid-cols-5 gap-4 items-center">
        {stats.map(([label, n, color]) => (
          <div key={label}>
            <div className="text-[10px] uppercase tracking-wider text-tm-muted">
              {label}
            </div>
            <div className={`text-2xl font-semibold mt-0.5 ${color}`}>{n}</div>
          </div>
        ))}
        <div className="text-right">
          <div className="text-[10px] uppercase tracking-wider text-tm-muted">
            kept rate
          </div>
          <div className="text-2xl font-semibold text-tm-text mt-0.5">
            {rate === null ? "—" : `${rate}%`}
          </div>
        </div>
      </div>
    </div>
  );
}

function CommitmentsTable({
  commitments,
  pendingId,
  onResolve,
  onSetState,
}: {
  commitments: CommitmentDto[];
  pendingId: string | null;
  onResolve: (id: string, polarity: number) => void;
  onSetState: (id: string, state: Exclude<CommitmentStateStr, "">) => void;
}) {
  if (commitments.length === 0) {
    return (
      <div className="text-sm text-tm-muted">
        No commitments in this window yet. Captures that read like "I'll …",
        "remind me to …", or "todo: …" get logged here automatically — or use{" "}
        <code className="text-tm-text">tracemind ledger</code> from the CLI.
      </div>
    );
  }
  return (
    <div className="space-y-2">
      {commitments.map((c) => (
        <CommitmentRow
          key={c.id}
          c={c}
          isPending={pendingId === c.id}
          onResolve={onResolve}
          onSetState={onSetState}
        />
      ))}
    </div>
  );
}

function CommitmentRow({
  c,
  isPending,
  onResolve,
  onSetState,
}: {
  c: CommitmentDto;
  isPending: boolean;
  onResolve: (id: string, polarity: number) => void;
  onSetState: (id: string, state: Exclude<CommitmentStateStr, "">) => void;
}) {
  const pillCls: Record<CommitmentStateStr, string> = {
    "": "bg-tm-muted/15 text-tm-muted",
    pending: "bg-tm-accent/15 text-tm-accent",
    kept: "bg-tm-green/15 text-tm-green",
    broken: "bg-tm-red/15 text-tm-red",
    abandoned: "bg-tm-muted/15 text-tm-muted",
  };
  const due = c.due_at ? new Date(c.due_at).toLocaleDateString() : "—";
  const overdue =
    c.state === "pending" && c.due_at !== null && c.due_at < Date.now();

  return (
    <div
      className={`rounded border ${
        overdue ? "border-tm-red/40" : "border-tm-border"
      } bg-tm-surface px-4 py-3`}
    >
      <div className="flex items-center gap-3">
        <span
          className={`text-xs px-1.5 py-0.5 rounded uppercase tracking-wider ${
            pillCls[c.state]
          }`}
        >
          {c.state || "—"}
        </span>
        <div className="flex-1">
          <div className="text-sm text-tm-text">{c.payload_ref}</div>
          <div className="text-xs text-tm-muted mt-0.5 font-mono">
            due {due} · {c.id.slice(0, 8)}
            {overdue ? " · overdue" : ""}
          </div>
        </div>
        {c.state === "pending" && (
          <div className="flex items-center gap-1">
            <button
              disabled={isPending}
              onClick={() => onResolve(c.id, 1.0)}
              className="px-2.5 py-1 text-xs rounded bg-tm-green/15 text-tm-green hover:bg-tm-green/25 disabled:opacity-40"
            >
              Kept
            </button>
            <button
              disabled={isPending}
              onClick={() => onResolve(c.id, -1.0)}
              className="px-2.5 py-1 text-xs rounded bg-tm-red/15 text-tm-red hover:bg-tm-red/25 disabled:opacity-40"
            >
              Broken
            </button>
            <button
              disabled={isPending}
              onClick={() => onSetState(c.id, "abandoned")}
              className="px-2.5 py-1 text-xs rounded border border-tm-border text-tm-muted hover:text-tm-text disabled:opacity-40"
            >
              Abandon
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
