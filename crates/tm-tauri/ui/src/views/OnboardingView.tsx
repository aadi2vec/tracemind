import { useState } from "react";
import { demoIngest, queryMemory, type QueryResponse } from "../api";

// UI-8 — first-run onboarding. Three steps: load demo fixture →
// run a starter query against it → land on the Brief. Designed to
// produce a *meaningful* response in ~60 seconds so the first-run
// experience matches the wedge sentence on the marketing page.

type Step = "intro" | "loading" | "queried" | "done";

interface Props {
  /// Fires when the user clicks "Open the brief" so the App can
  /// route them out of onboarding for good.
  onComplete: () => void;
}

const STARTER_QUERY = "What did I work on this week?";

export default function OnboardingView({ onComplete }: Props) {
  const [step, setStep] = useState<Step>("intro");
  const [stats, setStats] = useState<{ entities: number; triples: number; texts: number } | null>(null);
  const [result, setResult] = useState<QueryResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function loadSeed() {
    setStep("loading");
    setErr(null);
    try {
      const r = await demoIngest();
      setStats({ entities: r.total_entities, triples: r.total_triples, texts: r.texts_ingested });
      // Auto-run the starter query so the user sees a hit, not a
      // blank screen.
      try {
        const q = await queryMemory(STARTER_QUERY);
        setResult(q);
      } catch {
        /* leave result null; user can still proceed */
      }
      setStep("queried");
    } catch (e) {
      setErr(String(e));
      setStep("intro");
    }
  }

  return (
    <div className="max-w-2xl mx-auto py-10">
      <header className="mb-8 text-center">
        <h1 className="text-3xl font-semibold text-tm-text">Welcome to TraceMind</h1>
        <p className="text-tm-muted mt-2 text-sm">
          Ambient memory for every AI you use — captures what you do, scopes itself to the right
          context, learns your boundaries, never uploaded.
        </p>
      </header>

      {err && (
        <div className="bg-red-900/30 border border-red-700 rounded p-3 text-sm text-red-200 mb-6">
          {err}
        </div>
      )}

      {step === "intro" && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-6">
          <h2 className="text-lg font-medium text-tm-text">Step 1 — Seed a sample memory</h2>
          <p className="text-sm text-tm-muted mt-2">
            We'll load a small demo fixture (about 20 ingestion turns from a fictional founder's
            week). You'll then ask TraceMind a question about it and see what it answers, all in
            under a minute. <strong>Nothing is uploaded.</strong>
          </p>
          <button
            onClick={loadSeed}
            className="mt-5 px-4 py-2 bg-tm-accent text-white rounded hover:bg-tm-accent/80 transition-colors text-sm"
          >
            Load demo memory →
          </button>
          <p className="text-xs text-tm-muted mt-4">
            Prefer to start clean?{" "}
            <button onClick={onComplete} className="text-tm-accent hover:underline">
              Skip and use your own captures
            </button>
            .
          </p>
        </div>
      )}

      {step === "loading" && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-6 text-center">
          <div className="inline-block w-8 h-8 border-2 border-tm-accent border-t-transparent rounded-full animate-spin" />
          <p className="text-sm text-tm-muted mt-3">Seeding your memory…</p>
        </div>
      )}

      {step === "queried" && (
        <div className="space-y-4">
          <div className="bg-tm-surface border border-tm-border rounded-lg p-5">
            <h2 className="text-lg font-medium text-tm-text">Step 2 — Your first query</h2>
            {stats && (
              <p className="text-xs text-tm-muted mt-1">
                {stats.texts} memories ingested · {stats.entities} entities · {stats.triples}{" "}
                relationships
              </p>
            )}
            <div className="mt-4 bg-black/30 border border-tm-border rounded p-3">
              <p className="text-xs text-tm-muted mb-1">You asked</p>
              <p className="text-sm text-tm-text">"{STARTER_QUERY}"</p>
            </div>
            {result && (
              <div className="mt-3 bg-black/30 border border-tm-border rounded p-3">
                <p className="text-xs text-tm-muted mb-1">
                  TraceMind answered — {result.latency_ms} ms, arm: {result.arm_name}
                </p>
                {result.entities.length === 0 ? (
                  <p className="text-sm text-tm-muted italic">
                    No exact match (try the Query view for free-form questions).
                  </p>
                ) : (
                  <ul className="text-sm text-tm-text space-y-1">
                    {result.entities.slice(0, 5).map((e) => (
                      <li key={e.id}>
                        <span className="font-medium">{e.name}</span>{" "}
                        <span className="text-tm-muted text-xs">({e.entity_type})</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            <button
              onClick={() => setStep("done")}
              className="mt-5 px-4 py-2 bg-tm-accent text-white rounded hover:bg-tm-accent/80 transition-colors text-sm"
            >
              Next →
            </button>
          </div>
        </div>
      )}

      {step === "done" && (
        <div className="bg-tm-surface border border-tm-border rounded-lg p-6">
          <h2 className="text-lg font-medium text-tm-text">Step 3 — You're ready</h2>
          <p className="text-sm text-tm-muted mt-2">
            From here, three things happen automatically:
          </p>
          <ul className="text-sm text-tm-muted mt-3 space-y-1.5 list-disc list-inside">
            <li>
              <strong className="text-tm-text">Brief</strong> shows what to look at first each
              day (overdue commitments, contradictions, surprises).
            </li>
            <li>
              <strong className="text-tm-text">Ambient capture</strong> seeds itself from your
              shell history, Apple Notes, and clipboard (text-only, default on).
            </li>
            <li>
              <strong className="text-tm-text">Settings → Capture sources</strong> is where you
              control what's on. Default install touches no faces, no audio, no screenshots.
            </li>
          </ul>
          <div className="mt-5 flex gap-3">
            <button
              onClick={onComplete}
              className="px-4 py-2 bg-tm-accent text-white rounded hover:bg-tm-accent/80 transition-colors text-sm"
            >
              Open the brief
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
