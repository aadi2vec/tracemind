// WME-5 — Working Memory Engine card panel.
//
// Renders the verb-first cards (Resume / Recall / Compare / Caution /
// Connect / Anticipate) produced by tm-reflect::WorkingMemoryEngine.
// Each card has a 4-button feedback row:
//
//   useful · not useful · remind later · dismiss kind
//
// Feedback round-trips through cmd_wme_feedback which writes a row to
// wme_feedback + bumps the per-kind decayed outcome aggregator so the
// scorer can learn which kinds the user actually consumes.
//
// The panel is read-only against the WME — it does not trigger a
// producer tick. Cards appear once the ingest pipeline / brief loop
// pushes them.

import { useCallback, useEffect, useState } from "react";
import {
  getWmeCards,
  wmeFeedback,
  type WmeCard,
  type WmeCardKind,
  type WmeFeedback,
} from "../api";

const VERB_STYLE: Record<WmeCardKind, { label: string; cls: string }> = {
  resume: { label: "Resume", cls: "text-sky-400 border-sky-500/40 bg-sky-500/5" },
  recall: { label: "Recall", cls: "text-emerald-400 border-emerald-500/40 bg-emerald-500/5" },
  compare: { label: "Compare", cls: "text-violet-400 border-violet-500/40 bg-violet-500/5" },
  caution: { label: "Caution", cls: "text-amber-400 border-amber-500/40 bg-amber-500/5" },
  connect: { label: "Connect", cls: "text-fuchsia-400 border-fuchsia-500/40 bg-fuchsia-500/5" },
  anticipate: { label: "Anticipate", cls: "text-cyan-400 border-cyan-500/40 bg-cyan-500/5" },
};

interface WmeCardsPanelProps {
  /// Optional override for `getWmeCards(limit)`. Defaults to 12.
  limit?: number;
  /// Optional title override (defaults to "Working Memory").
  title?: string;
}

export default function WmeCardsPanel({ limit = 12, title = "Working Memory" }: WmeCardsPanelProps) {
  const [cards, setCards] = useState<WmeCard[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Optimistic hidden ids — keeps the card out of view immediately
  // after feedback so the panel doesn't flash the same item back in
  // the brief refresh window.
  const [hidden, setHidden] = useState<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    try {
      const next = await getWmeCards(limit);
      setCards(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [limit]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const onFeedback = async (card: WmeCard, feedback: WmeFeedback) => {
    // Optimistic hide first; the engine's cooldown will keep similar
    // cards from re-emerging immediately.
    setHidden((prev) => {
      const next = new Set(prev);
      next.add(card.id);
      return next;
    });
    try {
      await wmeFeedback(card.id, card.kind, feedback);
    } catch (e) {
      setError(String(e));
      // Roll back the optimistic hide on failure.
      setHidden((prev) => {
        const next = new Set(prev);
        next.delete(card.id);
        return next;
      });
    }
  };

  if (error) {
    return (
      <section className="mb-6">
        <h3 className="text-sm font-semibold text-rose-400 mb-2">{title}</h3>
        <p className="text-xs text-tm-muted">failed to load WME cards: {error}</p>
      </section>
    );
  }

  const visible = (cards ?? []).filter((c) => !hidden.has(c.id));
  if (visible.length === 0) {
    // Quiet when there are no cards — the WME may not have ticked yet
    // and we don't want to add noise to the brief.
    return null;
  }

  return (
    <section className="mb-6">
      <h3 className="text-sm font-semibold text-tm-text mb-2">
        {title}{" "}
        <span className="text-xs text-tm-muted font-normal">
          ({visible.length})
        </span>
      </h3>
      <p className="text-xs text-tm-muted mb-2">
        Verb-first action cards proposed by the working-memory engine.
        Feedback tunes which kinds get surfaced next.
      </p>
      <ul className="space-y-2">
        {visible.map((card) => {
          const style = VERB_STYLE[card.kind];
          return (
            <li
              key={card.id}
              className={`border rounded px-4 py-2.5 text-sm ${style.cls}`}
            >
              <div className="flex items-start gap-3">
                <span
                  className={`text-xs uppercase tracking-wide font-semibold ${style.cls} px-2 py-0.5 rounded`}
                >
                  {style.label}
                </span>
                <div className="flex-1">
                  <div className="text-tm-text">{card.statement}</div>
                  <div className="font-mono text-[11px] text-tm-muted mt-1">
                    score {card.score.toFixed(2)}
                    {" · "}rel {card.relevance.toFixed(2)}
                    {" · "}sur {card.surprise.toFixed(2)}
                    {" · "}rec {card.recency.toFixed(2)}
                    {" · "}out {card.outcome.toFixed(2)}
                    {" · "}
                    <span>{card.created_at}</span>
                  </div>
                </div>
              </div>
              <div className="mt-2 flex flex-wrap gap-2">
                <FeedbackButton
                  label="useful"
                  onClick={() => onFeedback(card, "useful_now")}
                />
                <FeedbackButton
                  label="not useful"
                  onClick={() => onFeedback(card, "not_useful_now")}
                />
                <FeedbackButton
                  label="remind later"
                  onClick={() => onFeedback(card, "not_now_remind_later")}
                />
                <FeedbackButton
                  label="dismiss kind"
                  onClick={() => onFeedback(card, "dismiss_this_kind")}
                  danger
                />
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function FeedbackButton({
  label,
  onClick,
  danger = false,
}: {
  label: string;
  onClick: () => void;
  danger?: boolean;
}) {
  const cls = danger
    ? "text-rose-400 border-rose-500/30 hover:bg-rose-500/10"
    : "text-tm-muted border-tm-border hover:text-tm-text hover:bg-tm-accent/10";
  return (
    <button
      onClick={onClick}
      className={`text-xs px-2 py-0.5 rounded border transition ${cls}`}
    >
      {label}
    </button>
  );
}
