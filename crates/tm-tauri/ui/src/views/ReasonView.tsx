import { useState } from "react";
import { reasonChain, reasonExplore, findAnalogies, type ReasoningChainInfo, type AnalogyInfo } from "../api";

type Mode = "explore" | "chain" | "analogy";

export default function ReasonView() {
  const [mode, setMode] = useState<Mode>("explore");
  const [input1, setInput1] = useState("");
  const [input2, setInput2] = useState("");
  const [chains, setChains] = useState<ReasoningChainInfo[]>([]);
  const [analogies, setAnalogies] = useState<AnalogyInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const handleSubmit = async () => {
    if (!input1.trim()) return;
    setLoading(true);
    setError("");
    setChains([]);
    setAnalogies([]);

    try {
      if (mode === "explore") {
        const res = await reasonExplore(input1.trim());
        setChains(res);
      } else if (mode === "chain") {
        if (!input2.trim()) { setError("Enter both entities"); setLoading(false); return; }
        const res = await reasonChain(input1.trim(), input2.trim());
        setChains(res);
      } else {
        const res = await findAnalogies(input1.trim());
        setAnalogies(res);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Reasoning Engine</h2>

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
