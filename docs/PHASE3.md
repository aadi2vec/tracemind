# Phase 3 — Tiered Answers, Local LLMs, and the Seamless Moat

**Window**: April 2026 → October 2026 (6 months)
**Goal**: ship the best local memory intelligence platform of all time.
**North-star metric**: LoCoMo ≥ 85, published, CI-gated.

---

## 0. Thesis

Consumers want a second brain that doesn't send their thoughts to a server. Apple
Intelligence (macOS Tahoe 26, 3B on-device FoundationModels) just removed the
last technical objection to "but where does the LLM run." The window to own
"local memory for the Apple ecosystem" is now. Honcho, Mem0, Letta, and Zep are
structurally cloud-native — privacy is a moat they cannot copy.

TraceMind wins by being (1) the best memory *substrate* that (2) works on every
Mac since 2018, (3) produces verifiable, cited answers from local weights, and
(4) gets out of the user's way.

---

## 1. The Tiered Answer Architecture

Three tiers, user-controlled, all sharing the same retrieval backbone.

| Tier | Engine | Install cost | Quality ceiling | Used for |
|---|---|---|---|---|
| 0 | Extractive only (template) | 0 MB | Baseline | Default OOB, retrieval + templated answers |
| 1 | Bundled 1B LLM (Qwen 2.5 1.5B Q4) | 800 MB opt-in | Good | Ingestion extraction, consolidation, contradictions, short Q&A |
| 2 | Apple FoundationModels (3B) | 0 MB on macOS 26 AS | Best | Open-ended synthesis, long-context answers |

**Selection policy** (automatic, user-overridable):
- macOS Tahoe 26 + Apple Silicon + answer is open-ended → Tier 2
- Tier 1 installed + structured task (extraction/contradiction/summary) → Tier 1
- Else → Tier 0

**RAM budget amendment to CLAUDE.md**:
- `<200 MB idle` when only Tier 0 enabled (current baseline)
- `<1.2 GB active` when Tier 1 inference is live; mmap'd + lazy-unload after 5 min idle
- Tier 2 inherits Apple's footprint; we don't count it

---

## 2. Model Selection

### Tier 1 candidates (ranked)
1. **Qwen 2.5 1.5B Instruct** — Apache 2.0, multilingual, strong extraction, ~900 MB Q4_K_M. **Primary choice.**
2. **Llama 3.2 1B Instruct** — Meta Community License, cleanest name recognition, ~700 MB Q4_K_M. **Fallback.**
3. **Gemma 3 1B** — Gemma ToU, slightly murky commercial clause, skip unless others regress.

### Inference engine
- **llama.cpp via `llama-cpp-2` Rust bindings**. Metal on Mac, CPU fallback on Intel/Linux/Windows, GGUF ecosystem, battle-tested.
- Alternatives evaluated and rejected for Phase 3: `mistral.rs` (narrower model support), `candle` (thin for full LLM at quality), `MLX` (Rust FFI painful, Apple-only).

### Tier 2 integration
- Apple FoundationModels via `objc2` + `objc2-foundation-models` when available.
- Feature-gated behind `cfg(target_os = "macos")` with runtime version check for macOS 26.
- Graceful fallback to Tier 1 if FM init fails.

---

## 3. Crate Plan

New:
- **`tm-answer`** — tiered answer layer. Exposes `AnswerBackend` trait with three impls (`ExtractiveBackend`, `LocalLlmBackend`, `AppleFmBackend`) + a `TieredAnswerer` that dispatches.
- **`tm-bench-locomo`** — LoCoMo + LongMemEval harness. CI-gated. Must land **before** any Tier 1 code merges.

Modified:
- `tm-retrieval` — emits `RetrievalContext` that the answerer consumes
- `tm-ingest` — optional structured extraction pass via Tier 1 at promotion time
- `tm-cli` — `tracemind ask "question"` subcommand
- `tm-mcp` — new `memory_answer` tool that returns cited synthesized answers
- `tm-tauri` — Tier 1 download/install UX, tray status

---

## 4. Six-Month Ship Plan

### Weeks 1–2: quality foundation
- **TM-5.2-005** LoCoMo harness lands. Published number in README. CI gate blocks merges that drop >0.5 points.
- **TM-5.2-001** Tauri .dmg ships signed. One-click install, auto-start, tray icon.

### Weeks 3–4: Apple FM bridge (zero install cost, ship first)
- `tm-answer` crate scaffolded with trait + extractive backend (Tier 0).
- Apple FoundationModels spike: 1-day prototype validates Rust → FM via objc2.
- `AppleFmBackend` lands behind `macos` feature flag.
- **TM-5.1-005** contradiction detection using Tier 2 when available.

### Weeks 5–6: Tier 1 local LLM
- `llama-cpp-2` integration in `tm-answer`.
- Qwen 2.5 1.5B Q4 bundled as opt-in download (hash-pinned, signed).
- First-run flow: "Enable local answers? [Download 900 MB]" — Ollama-style.
- Lazy-load + 5-min idle unload.
- **TM-5.1-002** structured MCP ingestion using Tier 1 at t2/t3 promotion.

### Weeks 7–8: distribution — Obsidian
- **TM-5.2-010** Obsidian plugin. ~2M addressable users, privacy-aware ICP.
- MCP schema v1 frozen before plugin ships.

### Weeks 9–10: offline dreaming
- **TM-5.1-004** consolidation daemon. Tier 1 rewrites, de-duplicates, promotes facts during idle.
- **TM-5.1-003** observation hierarchy + provenance chains (required for multi-hop temporal).

### Weeks 11–13: proactive surfacing + answer synthesis
- Global hotkey (⌘⇧Space) → search overlay → Tier 2/1 synthesized answer with citations.
- Proactive: "you're writing about X, here's what you knew."
- **TM-5.1-006** context budget + iterative retrieval.

### Weeks 14–18: capture breadth
- Chrome/Arc extension.
- **TM-5.3-006** micro-NER fine-tune on user's own consolidation traces.
- **TM-5.3-007** HNSW indexing in SQLite (sqlite-vec or custom).

### Weeks 19–26: multi-device + monetization
- iOS companion (read-only, iCloud relay).
- TraceMind Plus tier launches: $8/mo, E2E sync, iOS app, browser extensions.

---

## 5. Business Model

### Positioning
"Your memory, not theirs."

### Tiers
- **Free forever** — local core. Capture, store, search, answer. Unlimited, on-device.
- **Plus ($8/mo, month 5+)** — multi-device E2E sync, iOS, browser extensions, priority support.
- **Teams ($20/seat/mo, post-Phase-3)** — deferred; revisit month 9+.

### No API billing in Phase 3
Do not compete with Honcho/Mem0 on $/M tokens. Our local API is localhost-only
(Tauri ↔ local daemon). Letting cloud APIs compete on our terms loses.

### 6-month revenue target
50k downloads → 5k DAU → 2–5% Plus conversion = ~100–250 subscribers → $800–2k MRR.
**This is a signal, not a business.** Real revenue is months 9–18.

---

## 6. Product Pillars (ordered)

1. **Trust** — local-only, OSS core, auditable traces
2. **Quality** — LoCoMo ≥ 85 in README, regression-gated in CI
3. **Seamlessness** — tray app, hotkey, Obsidian, cited Apple FM / local 1B answers
4. **Capture breadth** — clipboard ✅ → browser → Obsidian → email → Messages

### Explicit non-goals for Phase 3
- Enterprise ACL / multi-tenant
- Self-hosted cloud version
- Windows / Linux ports (defer until post-Mac PMF)
- Custom LLM pre-training
- Competing on $/M tokens

---

## 7. Tech Strategy

### Architectural bets
1. **SQLite remains the only durable store.** HNSW via sqlite-vec covers us to 1M entities.
2. **Consolidation is an actual tokio daemon** with a priority queue. Two-speed pipeline's slow side gets real.
3. **Structured ingestion = Tier 1 LLM extracting entities + relations + contradictions at promotion.**
4. **Bench-driven development.** LoCoMo + DMR + LongMemEval in CI. No merges on >0.5 regression.
5. **Hybrid search**: BGE + BM25 + ColBERT → RRA fusion. Finish what's started.
6. **Provenance as first-class**: every synthesized answer must cite trace IDs. Differentiator vs Mem0/Honcho black boxes.

### Tech debt to pay down
- tm-tauri bundler models/ dir bug → blocks .dmg ship
- Stashed TM-4.0-004 perf benchmark → unblock after LoCoMo lands
- MCP tool schema stability → freeze v1 before Obsidian plugin

---

## 8. Competitive Landscape Snapshot (April 2026)

| System | LoCoMo | Deployment | Pricing | Strength | Weakness |
|---|---|---|---|---|---|
| Honcho 3 | SOTA | Cloud | $2/M tok | Dreaming, dialectic model | Cloud-only, $$ |
| Mem0 | 91.6 | Cloud | SaaS | <7k tok/retrieval | Black-box |
| Engram | 80 / 92% DMR | Cloud | SaaS | Graph + DMR | Cloud |
| SuperLocalMemory (Mode C) | 87.7 | Local | OSS | Proof local ≥ cloud | DIY UX |
| Letta | 74.0 | Cloud | SaaS | MemGPT lineage | Older |
| Zep | 75.14 | Cloud | SaaS | Temporal KG | Cloud |
| **TraceMind (target)** | **≥ 85** | **Local** | **$0 core** | **Privacy + Mac-native + cited answers** | **No published number yet** |

### Existential threats
- **Apple Intelligence** turns "local LLM" into a commodity. Answer: use FM as Tier 2, own the memory substrate.
- **Ollama + second-brain wrappers** (Fenn, Dottie, Raycast memory) can close UX gap fast. Answer: ship Tauri + Obsidian early, ship LoCoMo number early.
- **Honcho consumer app** if they ever ship one. Answer: privacy moat + Mac-native seamlessness + citations.

---

## 9. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Tier 1 regresses LoCoMo on model swap | High | High | CI gate on LoCoMo, hash-pin weights, prompt-eval harness |
| 800 MB download deters install | Med | Med | Opt-in flow, clear size communication, Tier 0 stays viable |
| Apple changes FM API in 26.x | Med | Med | Feature-flag, fallback to Tier 1, integration tests on beta |
| Intel Mac CPU inference too slow | High | Low | Document as "slow on Intel," default to Tier 0 there |
| LoCoMo ≥ 85 unreachable in 6 months | Med | High | Ship whatever number we hit, publish transparently, iterate |
| Model licensing changes (Llama, Qwen) | Low | High | Dual-candidate strategy, Apache-2.0 Qwen as primary |

---

## 10. First Two Weeks — Concrete

1. ✅ `docs/PHASE3.md` (this doc) committed.
2. Scaffold `crates/tm-answer` with `AnswerBackend` trait + `ExtractiveBackend`.
3. Scaffold `crates/tm-bench-locomo` with harness skeleton + sample dataset loader.
4. Amend `CLAUDE.md` with tiered RAM budget and new crates.
5. **Gate**: LoCoMo harness produces a number before any Tier 1 code merges.
6. **Gate**: signed .dmg ships before Obsidian plugin ships.

---

## 11. Success Criteria at Month 6

- ✅ LoCoMo ≥ 85 published in README, CI-gated
- ✅ Signed .dmg with auto-start + tray
- ✅ Tier 0 + Tier 1 + Tier 2 all shipped, user-switchable
- ✅ Obsidian plugin live, ≥ 5k installs
- ✅ 50k total downloads, 5k DAU
- ✅ Plus tier live, ≥ 100 paying subscribers
- ✅ Zero telemetry, zero cloud dependency for core flows
- ✅ Every answer is cited with trace IDs

If we hit 5 of 8 we're on track. If we hit 7+ we have a real product.
