// LM-5a — inline transclusion renderer.
//
// Takes a raw string that may contain `![[entity_id]]` or
// `![[Entity Name]]` markers and renders it as a normal text node
// with resolved markers replaced by clickable entity chips. Unresolved
// markers fall back to a muted "?" pill.

import { useEffect, useMemo, useState } from "react";
import { resolveTransclusion, type TransclusionSpan } from "../api";

type Segment =
  | { kind: "text"; text: string }
  | { kind: "span"; span: TransclusionSpan };

export default function TransclusionText({
  text,
  onOpenEntity,
  className,
}: {
  text: string;
  onOpenEntity?: (entityIdOrName: string) => void;
  className?: string;
}) {
  const [spans, setSpans] = useState<TransclusionSpan[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Cheap pre-check: skip the IPC roundtrip entirely if the text has
  // no transclusion marker. Saves ~1ms per row in BriefView.
  const hasMarker = useMemo(() => text.includes("![["), [text]);

  useEffect(() => {
    if (!hasMarker) {
      setSpans([]);
      return;
    }
    let cancelled = false;
    resolveTransclusion(text)
      .then((s) => {
        if (!cancelled) setSpans(s);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [text, hasMarker]);

  if (!hasMarker || spans.length === 0) {
    return <span className={className}>{text}</span>;
  }
  if (error) {
    return (
      <span className={className} title={error}>
        {text}
      </span>
    );
  }

  const segments: Segment[] = [];
  let cursor = 0;
  for (const span of spans) {
    if (span.start > cursor) {
      segments.push({ kind: "text", text: text.slice(cursor, span.start) });
    }
    segments.push({ kind: "span", span });
    cursor = span.end;
  }
  if (cursor < text.length) {
    segments.push({ kind: "text", text: text.slice(cursor) });
  }

  return (
    <span className={className}>
      {segments.map((seg, i) =>
        seg.kind === "text" ? (
          <span key={i}>{seg.text}</span>
        ) : (
          <TransclusionChip
            key={i}
            span={seg.span}
            onOpenEntity={onOpenEntity}
          />
        ),
      )}
    </span>
  );
}

function TransclusionChip({
  span,
  onOpenEntity,
}: {
  span: TransclusionSpan;
  onOpenEntity?: (entityIdOrName: string) => void;
}) {
  if (!span.entity_id) {
    return (
      <span
        className="inline-block px-1.5 py-0.5 mx-0.5 text-[10px] uppercase tracking-wider rounded bg-rose-500/10 text-rose-300 border border-rose-500/30"
        title="transclusion target not found"
      >
        ?{" "}
        {span.entity_name ?? "unresolved"}
      </span>
    );
  }
  const target = span.entity_id;
  const handler = onOpenEntity
    ? () => onOpenEntity(target)
    : undefined;
  return (
    <button
      type="button"
      onClick={handler}
      disabled={!handler}
      title={span.preview ?? undefined}
      className="inline-block px-1.5 py-0.5 mx-0.5 text-xs rounded bg-tm-accent/10 text-tm-accent border border-tm-accent/30 hover:bg-tm-accent/20 transition disabled:cursor-default"
    >
      {span.entity_name}
    </button>
  );
}
