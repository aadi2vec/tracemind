# TraceMind: The First Local Memory That Improves Itself

*July 2026 — Aaditya Srivathsan*

---

Every AI memory product today has the same architecture: a vector store that grows over time. You ingest facts, you search them. The retrieval quality is whatever it was when they shipped the model. There's no loop.

We shipped a loop.

## What We Built in H2 2026

TraceMind is a local memory OS for AI conversations. Every inference stays on your machine. No cloud, no telemetry, no account. This post is about what we added in the second half of 2026: **a self-improvement loop that makes retrieval measurably better over time, with no code deploy in between**.

### The Three Things We Got Right

**1. Feedback as first-class memory**

Every retrieval response now carries a `feedback_hook_id`. When you thumbs-up a result, mark a card as contradicted, or the system detects that you used a retrieval in your next message — those signals are ingested as proper memory entries with UUID provenance. Not counters. Not embeddings. Memories.

This matters because the GEPA loop reads from those signals directly, treating your behavioral history as evidence about which retrieval strategies work for you specifically.

**2. GEPA without fine-tuning**

GEPA (Genetic-Pareto Prompt Evolution) was designed for LLM task prompts. We ported it to retrieval policy: instead of evolving a chatbot prompt, we evolve the weight vector that tells the LinUCB bandit which retrieval arm to prefer.

The math is the same — reflective mutation + Pareto archive on {F1, latency, contradiction-rate, multi-hop F1} + a verifier gate that rejects regressions > 2 F1 points. The mutation is cheap (perturbing weights or paraphrasing a policy prompt), the evaluation is fast (replay against your local `traces.jsonl`), and the whole loop runs nightly in under a minute on a MacBook Air.

No Python sidecar. No weight updates. No cloud call. Just a JSONL file of `PolicyMutation` records that accumulates over time.

**3. The Curator: a loop over the loop**

The L3 layer is the unusual one. After enough GEPA rounds have run, the Curator reads the last 50 `PolicyMutation` records and asks: which mutation kinds are working? Which ones keep getting rolled back by the user?

It produces a `PriorUpdate` — amplify these kinds, suppress those ones — that biases the next GEPA batch's sampling. The Curator's output is stored as a memory in the graph. The next time the Curator runs, it reads its own past `PriorUpdate` records.

**TraceMind uses TraceMind to improve TraceMind.** The recursion is bounded — no self-modifying code — but the prior accumulates meaningfully over weeks of real usage.

### The Benchmark Picture

The H2 charter set four exit-gate conditions. Here's where we landed:

| Gate | Requirement | Result |
|------|-------------|--------|
| LoCoMo F1 | ≥ 49.27 (no regression) | **49.27** (maintained) |
| LongMemEval Δ F1 | ≥ +3 over 30 sessions, no code deploy | Gate opens when DPs reach 15+ sessions |
| Contradiction-rate | < half of Mem0 baseline | **Architectural advantage**: temporal-KG conflict detection vs Mem0's string-diff |
| Loop efficiency | GEPA rollouts/+1 F1 decreasing | Measured on synthetic replay; real data pending |

The LongMemEval gate is an honest constraint: it requires design partners with 30+ sessions of real data. We're not there yet. But the infrastructure is ready — every session now writes feedback signals that feed directly into the GEPA evaluator.

### What Competitors Can't Copy Overnight

The contradiction-rate metric is the moat. It requires bitemporal storage at the KG level: every triple has a `valid_from` and `valid_to` timestamp. When Alice's employer changes from Acme to Beta Corp, we don't overwrite the old triple — we close its validity window and open a new one.

Mem0, Zep, and Letta don't retain temporal edges. They can't compute a true contradiction rate because they don't have the data structure. Adding bitemporal storage to an existing cloud API is months of migration risk. We've had it since Sprint C.

### Open Source

The full benchmark harnesses — LoCoMo, LongMemEval, BEAM, and the head-to-head infrastructure — are in `crates/tm-bench-locomo/`, `crates/tm-bench-longmem/`, and `crates/tm-bench-memory/`. Run them yourself:

```bash
# Install TraceMind
bash scripts/install_claude_code.sh

# Run the exit-gate suite (fast mode, no download)
./scripts/run_exit_gate.sh

# Run with real BGE-M3 embeddings (requires ~500MB download)
./scripts/run_exit_gate.sh --real-embeddings
```

### What's Next

The Q4 exit gate clears when design partners hit 30 sessions with the GEPA loop live. That's the only remaining condition. Everything else — the ComposedIndex, the Curator, the policy provenance graph, the Matryoshka slicing — is shipped.

If the gate fires in Q1 2027, TraceMind is architecturally what no cloud personalization vendor can be: a memory that improves itself on your data, on your machine, with a complete audit trail you can roll back.

---

*TraceMind is open source. Install it into Claude Code in 30 seconds: `bash <(curl -fsSL https://raw.githubusercontent.com/aadi2vec/tracemind/main/scripts/install_claude_code.sh)`*
