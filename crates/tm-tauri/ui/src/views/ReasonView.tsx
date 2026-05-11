import { useEffect, useRef, useState } from "react";
import {
  reasonChain,
  reasonExplore,
  findAnalogies,
  reasonSeed,
  type ReasoningChainInfo,
  type AnalogyInfo,
  type ReasonSeed,
} from "../api";

type Mode = "explore" | "chain" | "analogy";

export default function ReasonView() {
  const [mode, setMode] = useState<Mode>("explore");
  const [input1, setInput1] = useState("");
  const [input2, setInput2] = useState("");
  const [chains, setChains] = useState<ReasoningChainInfo[]>([]);
  const [analogies, setAnalogies] = useState<AnalogyInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  // 2026-05-11 UX (#2) — cold-start seed banner. Captures *which*
  // entity we auto-loaded so the user can either accept the suggestion
  // or type their own. `null` until seed lookup finishes; if no seed
  // is available (empty graph) we render the legacy empty state.
  const [seed, setSeed] = useState<ReasonSeed | null>(null);
  const seededOnceRef = useRef(false);

  // Run a query for the given entity name in the current mode.
  // Pulled out of `handleSubmit` so the cold-start effect below can
  // call it without going through the input → submit cycle.
  const runFor = async (name: string) => {
    setLoading(true);
    setError("");
    setChains([]);
    setAnalogies([]);
    try {
      if (mode === "explore") {
        setChains(await reasonExplore(name));
      } else if (mode === "chain") {
        if (!input2.trim()) {
          setError("Enter both entities");
          return;
        }
        setChains(await reasonChain(name, input2.trim()));
      } else {
        setAnalogies(await findAnalogies(name));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const handleSubmit = async () => {
    if (!input1.trim()) return;
    await runFor(input1.trim());
  };

  // Cold-start: on first mount, ask the backend for a meaningful seed
  // entity (highest PageRank in the active context) and auto-run
  // `explore` so the view never sits empty. Only fires once per
  // session — re-entering the view after a manual query keeps the
  // user's last result.
  useEffect(() => {
    if (seededOnceRef.current) return;
    seededOnceRef.current = true;
    (async () => {
      try {
        const s = await reasonSeed();
        if (s) {
          setSeed(s);
          setInput1(s.entity_name);
          // Auto-run in explore mode only — chain mode needs two
          // inputs, analogy mode is the user's choice.
          if (mode === "explore") {
            await runFor(s.entity_name);
          }
        }
      } catch (e) {
        console.error("reason seed failed:", e);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Reasoning Engine</h2>

      {/* Cold-start seed banner (2026-05-11 UX #2). Surfaces which
          entity we auto-seeded and why, so the user has context
          before deciding to type their own. */}
      {seed && (
        <div className="flex items-center gap-2 text-xs text-tm-muted bg-tm-surface border border-tm-border rounded-lg px-3 py-2">
          <span className="text-tm-accent">●</span>
          <span>
            Auto-seeded with <span className="text-tm-text font-medium">{seed.entity_name}</span>
            <span className="text-tm-muted"> — {seed.why}</span>
          </span>
          <button
            onClick={() => { setSeed(null); setInput1(""); setChains([]); setAnalogies([]); }}
            className="ml-auto text-[10px] uppercase tracking-wider text-tm-muted hover:text-tm-text transition-colors"
            title="Clear and start fresh"
          >
            clear
          </button>
        </div>
      )}

      {/* Mode tabs */}
      <div className="flex gap-2">
        {(["explore", "chain", "analogy"] as Mode[]).map((m) => (
          <button
            key={m}
            onClick={() => { setMode(m); setChains([]); setAnalogies([]); setError(""); }}
            className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
              mode === m
                ? "bg-tm-accent text-white"
                : "bg-tm-surface border border-tm-border text-tm-muted hover:text-tm-text"
            }`}
          >
            {m === "explore" ? "Explore Entity" : m === "chain" ? "Find Path" : "Find Analogies"}
          </button>
        ))}
      </div>

      {/* Input */}
      <div className="flex gap-3">
        <input
          className="flex-1 bg-tm-surface border border-tm-border rounded-lg px-4 py-3 text-sm placeholder-tm-muted"
          placeholder={mode === "chain" ? "Source entity..." : mode === "analogy" ? "Find entities like..." : "Explore from entity..."}
          value={input1}
          onChange={(e) => setInput1(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
        />
        {mode === "chain" && (
          <input
            className="flex-1 bg-tm-surface border border-tm-border rounded-lg px-4 py-3 text-sm placeholder-tm-muted"
            placeholder="Target entity..."
            value={input2}
            onChange={(e) => setInput2(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
          />
        )}
        <button
          onClick={handleSubmit}
          disabled={loading}
          className="px-6 py-3 bg-tm-accent text-white rounded-lg text-sm font-medium hover:bg-tm-accent/80 transition-colors disabled:opacity-50"
        >
          {loading ? "Thinking..." : "Reason"}
        </button>
      </div>

      {error && <div className="text-red-400 text-sm">{error}</div>}

      {/* Reasoning chains */}
      {chains.length > 0 && (
        <div className="space-y-4">
          <p className="text-xs text-tm-muted">{chains.length} reasoning path{chains.length > 1 ? "s" : ""} found</p>
          {chains.map((chain, ci) => (
            <div key={ci} className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <div className="flex items-center justify-between mb-3">
                <span className="text-xs font-medium text-tm-accent">
                  Path {ci + 1}
                </span>
                <span className="text-xs text-tm-muted">
                  score: {(chain.score * 100).toFixed(1)}% &middot; {chain.steps.length} hop{chain.steps.length > 1 ? "s" : ""}
                </span>
              </div>
              <div className="flex items-center flex-wrap gap-1">
                {chain.steps.map((step, si) => (
                  <div key={si} className="flex items-center gap-1">
                    {si > 0 && (
                      <span className="px-2 py-0.5 rounded bg-tm-accent/20 text-tm-accent text-[10px] font-mono">
                        {step.predicate}
                        {step.direction === "Backward" ? " ←" : " →"}
                      </span>
                    )}
                    <span className={`px-2.5 py-1 rounded-full text-xs font-medium ${
                      si === chain.steps.length - 1
                        ? "bg-emerald-500/20 text-emerald-400 ring-1 ring-emerald-500/30"
                        : "bg-tm-border/50 text-tm-text"
                    }`}>
                      {step.entity_name}
                      <span className="ml-1 opacity-50 text-[10px]">{step.entity_type}</span>
                    </span>
                  </div>
                ))}
              </div>
              {/* Confidence breakdown */}
              <div className="mt-3 flex gap-3">
                {chain.steps.map((step, si) => (
                  <div key={si} className="flex items-center gap-1 text-[10px] text-tm-muted">
                    <div className="w-8 h-1 bg-tm-border rounded-full overflow-hidden">
                      <div
                        className="h-full bg-tm-accent rounded-full"
                        style={{ width: `${step.confidence * 100}%` }}
                      />
                    </div>
                    <span>{(step.confidence * 100).toFixed(0)}%</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Analogies */}
      {analogies.length > 0 && (
        <div className="space-y-3">
          <p className="text-xs text-tm-muted">{analogies.length} analog{analogies.length > 1 ? "ies" : "y"} found</p>
          {analogies.map((a, i) => (
            <div key={i} className="bg-tm-surface border border-tm-border rounded-lg p-5">
              <div className="flex items-center gap-3 mb-2">
                <span className="text-sm font-medium">{a.source_name}</span>
                <span className="text-tm-muted text-xs">is like</span>
                <span className="text-sm font-medium text-emerald-400">{a.target_name}</span>
                <span className="text-xs text-tm-muted ml-auto">
                  {(a.similarity * 100).toFixed(0)}% similar
                </span>
              </div>
              <p className="text-xs text-tm-muted">{a.explanation}</p>
              {a.shared_patterns.length > 0 && (
                <div className="flex flex-wrap gap-1 mt-2">
                  {a.shared_patterns.map((p, pi) => (
                    <span key={pi} className="px-2 py-0.5 rounded bg-tm-accent/10 text-tm-accent text-[10px] font-mono">
                      {p}
                    </span>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Empty state */}
      {!loading && chains.length === 0 && analogies.length === 0 && !error && (
        <div className="text-center py-12 text-tm-muted">
          <p className="text-sm">
            {mode === "explore"
              ? "Enter an entity name to explore its reasoning neighborhood"
              : mode === "chain"
              ? "Enter two entity names to find reasoning paths between them"
              : "Enter an entity name to find structurally similar entities"}
          </p>
        </div>
      )}
    </div>
  );
}
