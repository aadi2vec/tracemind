// CTX-EVG-C — homepage Commitment Ledger score card.
//
// Compact 4-number summary embedded in the Brief. Clicking jumps to
// the full Ledger surface. Stays silent when there's nothing in the
// window (no commitments yet) — surface real-estate rule from the
// outcome-bond memory note: feedback loops earn space, decoration
// does not.

import { useEffect, useState } from "react";
import { commitmentLedger, type LedgerDto } from "../api";

export default function LedgerScoreCard({
  windowDays = 7,
  onOpenLedger,
}: {
  windowDays?: number;
  onOpenLedger?: () => void;
}) {
  const [data, setData] = useState<LedgerDto | null>(null);
  const [err, setErr] = useState<string>("");

  useEffect(() => {
    commitmentLedger(windowDays)
      .then(setData)
      .catch((e) => setErr(String(e)));
  }, [windowDays]);

  if (err || data === null) return null;

  const total = data.kept + data.broken + data.pending + data.abandoned;
  if (total === 0) return null;

  const decided = data.kept + data.broken;
  const rate = decided === 0 ? null : Math.round((data.kept / decided) * 100);

  return (
    <button
      onClick={onOpenLedger}
      className="w-full mb-5 text-left rounded-lg border border-tm-border bg-tm-surface hover:border-tm-accent/40 transition-colors px-5 py-3"
    >
      <div className="flex items-center justify-between mb-1.5">
        <div className="text-xs uppercase tracking-wider text-tm-muted">
          commitment ledger · last {windowDays}d
        </div>
        <div className="text-xs text-tm-muted">open →</div>
      </div>
      <div className="grid grid-cols-5 gap-3 items-center">
        <Stat label="kept" n={data.kept} color="text-tm-green" />
        <Stat label="broken" n={data.broken} color="text-tm-red" />
        <Stat label="pending" n={data.pending} color="text-tm-accent" />
        <Stat label="abandoned" n={data.abandoned} color="text-tm-muted" />
        <Stat
          label="kept rate"
          n={rate === null ? "—" : `${rate}%`}
          color="text-tm-text"
        />
      </div>
    </button>
  );
}

function Stat({
  label,
  n,
  color,
}: {
  label: string;
  n: number | string;
  color: string;
}) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wider text-tm-muted">
        {label}
      </div>
      <div className={`text-xl font-semibold mt-0.5 ${color}`}>{n}</div>
    </div>
  );
}
