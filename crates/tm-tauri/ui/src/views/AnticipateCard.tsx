// LGM-2 — "Anticipate" verb card.
//
// Reads PGM marginals via `cmd_anticipate` and renders them as a small
// "what's likely next?" panel. This is the user-facing payoff for
// EVG→LGM: the event graph feeds the Bayesian network, the network
// gives us conditioned forecasts, and this card surfaces them.
//
// Self-contained: no props required. The card derives a sensible
// default evidence map (current `time_window` bucket) and predicts
// the next `event_kind`.

import { useEffect, useMemo, useState } from "react";
import { anticipate, type AnticipateRowDto } from "../api";

function timeWindowBucket(d: Date): string {
  const h = d.getHours();
  if (h >= 5 && h <= 11) return "morning";
  if (h >= 12 && h <= 17) return "afternoon";
  if (h >= 18 && h <= 22) return "evening";
  return "night";
}

export default function AnticipateCard() {
  const [rows, setRows] = useState<AnticipateRowDto[]>([]);
  const [err, setErr] = useState<string>("");
  const [loaded, setLoaded] = useState(false);

  const evidence = useMemo(
    () => ({ time_window: timeWindowBucket(new Date()) }),
    [],
  );

  useEffect(() => {
    anticipate("event_kind", evidence, 3)
      .then((rs) => {
        setRows(rs);
        setErr("");
      })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoaded(true));
  }, [evidence]);

  if (!loaded) return null;

  // Don't render a card with nothing to say — the PGM hasn't seen
  // enough data yet.
  const haveSignal = rows.some((r) => r.support >= 3);
  if (!haveSignal && !err) return null;

  return (
    <section className="mb-6 border border-tm-border rounded-lg p-4 bg-tm-surface/50">
      <div className="flex items-baseline justify-between mb-2">
        <h3 className="text-sm font-semibold text-tm-text">
          🔮 Anticipate
        </h3>
        <span className="text-[10px] uppercase tracking-wide text-tm-muted">
          given {evidence.time_window}
        </span>
      </div>
      {err ? (
        <p className="text-xs text-rose-400" role="alert">
          {err}
        </p>
      ) : (
        <ul className="space-y-1">
          {rows.map((r) => (
            <li
              key={r.value}
              className="flex items-baseline gap-3 text-sm"
            >
              <span className="font-mono text-xs text-tm-muted w-12 text-right">
                {(r.probability * 100).toFixed(0)}%
              </span>
              <span className="text-tm-text">{r.value}</span>
              <span className="text-xs text-tm-muted ml-auto">
                {r.support} obs
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="text-[10px] text-tm-muted mt-2 leading-relaxed max-w-md">
        Bayesian forecast over your event graph — what kind of thing
        usually happens at this time of day.
      </p>
    </section>
  );
}
