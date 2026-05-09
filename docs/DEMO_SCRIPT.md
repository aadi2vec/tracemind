# TraceMind — Recordable Demo Script

**Owner:** Aaditya. **Audience:** investors / first design partners. **Length target:** 3:00 ± 0:15.
**Status:** living script. Updated as D-2 → D-6 ship.

The demo's hook is the **retraction beat**: TraceMind shows the user a contradiction it found in their own knowledge graph and lets them resolve it. This is the only thing on screen that no other "AI memory" product on the market can do, and it became real in Sprint C-2.

---

## Pre-roll (off camera)

```bash
tracemind demo restore       # D-2 — drops a deterministic ~/.tracemind/
tracemind capture start &    # D-3 — capture daemon runs silently
sleep 30                     # D-3 — 30s of clipboard / shell-history capture
```

The fixture (`crates/tm-cli/fixtures/demo/`) ships:

- 60 entities, 75 triples, 1 ready-made contradiction (`loves` vs `hates` on the same subject/object)
- 2 due commitments (one due *today*, one *3d overdue*)
- 2 stale commitments (>14d, kept around so the brief's stale section isn't empty)
- 6 resolved commitments in the last 7d (so insights/patterns have priors to draw on)

---

## Shot 1 — Cold open (0:00 – 0:25)

**On screen:** Tauri app. No browser, no terminal.

**Voiceover:** "Every memory product I've tried is a search box over my notes. TraceMind is different — it has opinions about what I should look at this morning."

**Action:** open the Tauri app. The brief panel (D-4) is already rendered.

```
TRACEMIND BRIEF                    Mon May 11, 8:42 AM
  overdue: 2    open: 5    resolved: 6    contradictions: 1

▸ contradictions (1)
    ⚡ "Alice loves Bob"  vs  "Alice hates Bob"
        cosine -0.94    detected 2 days ago
        → review

▸ overdue (2)
    9f3a  [due today]   by May 11    file Q1 board memo with Carla
    1c0e  [overdue]     by May 8     follow up on Mercury contract
```

---

## Shot 2 — The retraction beat (0:25 – 1:15)

**Voiceover:** "It noticed I told it two contradicting things about Alice and Bob — once in March, once last week. It's been holding both, downranking them, and asking me which one is true."

**Action:** click the contradiction row → drawer opens with both source captures + the ingest dates.

```
Alice loves Bob          ingested  Mar 14, from Slack DM with Priya
Alice hates Bob          ingested  May 6, from a meeting note

[ keep loves ]   [ keep hates ]   [ keep both — they're about different times ]
```

**Action:** click *keep hates*. Brief refreshes; `loves` is now `Out`, removed from retrieval.

**Voiceover:** "That's not search. That's a system that remembers, contradicts itself, and asks me to settle it. Now everything I retrieve about Alice and Bob is consistent with the version I just confirmed."

---

## Shot 3 — Why the brief is grounded (1:15 – 1:50)

**Voiceover:** "While we were talking, TraceMind was watching my clipboard."

**Action:** copy a paragraph from a real source (the pre-roll capture daemon picks it up). Wait two seconds. Click *refresh brief*.

A new row appears in the brief's *captured today* section — the just-captured paragraph is now an entity in the graph, with the right entity type and a confidence score.

**Voiceover:** "No upload, no cloud, no toggle. The signal lake is local — `~/.tracemind/`. The whole product is."

---

## Shot 4 — The overdue intent + outcome prompt (1:50 – 2:30)

**Voiceover:** "Back to the brief. The 'file Q1 board memo with Carla' row is overdue."

**Action:** click the row → drawer shows the outcome prompt: *"Did you ship it? What was the outcome?"*

**Action:** mark it **completed** + **positive**. The brief refreshes — the row leaves the *overdue* section, lands in *resolved*, and a *pattern* row updates: *"Friday-evening commitments: 5/6 shipped on time."*

**Voiceover:** "Every commitment closes with an outcome. That outcome trains a small local world model that's already calling Friday-evening intents 'high-completion'. Tomorrow's brief will tell me whether the new commitment I'm about to make fits the pattern."

---

## Shot 5 — Product close (2:30 – 3:00)

**Voiceover:** "One engine, three products."

**On screen** (D-6 one-pager):

```
TRACEMIND     personal memory OS    →   the product on screen now
ENGRAM        memory SDK            →   what agents and apps embed
ROSETTA       semantic code memory  →   what teams use over their repo
```

**Voiceover:** "All three share `tm-graph`, `tm-tms`, `tm-temporal`, the same JTMS, the same bitemporal substrate. Local-only by default. Fine-tuneable on-device. Ship it on your laptop, your phone, your team's repo."

**End card:** `tracemind.dev` · `cargo install tracemind`

---

## Hard requirements for the recording

- **D-2 fixture must restore deterministically** — same 60 entities, same contradiction, same horizons relative to *today*. Recording must be re-shootable.
- **D-3 capture pre-roll must be silent** — no terminal flicker, no permission prompts. Daemon already running before camera rolls.
- **D-4 Tauri brief panel must surface the contradictions row** — CLI brief output is fallback only.
- **D-5 install one-liner must be on the end card** — even if not yet live, it ships in the next sprint.
- Voice is Aaditya's, not synthesized. Fewer adjectives, fewer "imagine if" sentences.
- One take. If a beat needs three takes, fix the product, not the script.

---

## What's *not* in the demo (and why)

- LoCoMo F1 numbers — investors care about the retraction beat, not benchmarks. Numbers go in the deck, not the demo.
- The MCP / Claude Code integration — important for the developer audience, separate 90s clip.
- Reasoning chains, analogy solver — too abstract for a 3-minute first impression.
- Voice mode — ships later (P5). Demo is text + click only.
- Tier-1 LLM synthesis — works in the brief but not visually distinguishable from extractive in a 3-minute take. Save for the dev clip.
- LoRA fine-tuning (P11) — explicitly off the demo path; mention in the deck.
