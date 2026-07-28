# TraceMind — 10-week validation plan

**Date:** 2026-07-28 · **Target:** Winter 2027 YC deadline (~late Oct / early Nov)
**Turns:** `Are people using your product? **No**` → a real retention number.
**Companion to:** `YC-READINESS-2026-07-28.md` §6

The single goal: **≥3 design partners, ≥15 sessions each, and a day-14
retention number.** Everything below serves that. Nothing else ships.

---

## 0. The blocker nobody has noticed

`usage.json` — `first_seen`, `active_days`, `total_queries`, `total_helpful`,
`total_negative` — **already exists and is exactly the right instrument.** It's
local, privacy-preserving, and needs no new design.

**It is written by `tm-tauri` only.** The MCP server writes nothing:

```
$ grep -rln "usage.json" crates/*/src/
crates/tm-tauri/src/main.rs        ← the desktop app. P5. "not the front door."
```

Your distribution thesis is *MCP into Claude Code*. The desktop app is the one
surface you've said isn't the product. **So the instrumentation exists on the
surface nobody uses and is absent on the surface everybody would use.** Onboard
five partners tomorrow and you'd learn nothing.

Also: `tracemind share-usage --to <email>`, referenced in the `usage.json`
comment at `crates/tm-tauri/src/main.rs:5583`, **does not exist**. There is no
way for a partner to send you their numbers.

**These two gaps are the whole of Week 0. Nothing else matters until they close.**

---

## Week 0 (this week) — make measurement possible

| # | Task | Where | Done when |
|---|---|---|---|
| 0.1 | Extract `UsageStats` + load/save into a shared crate (`tm-episodic` or a small `tm-usage`) | `tm-tauri` → shared | both binaries link one impl |
| 0.2 | Bump usage on every `memory_query` / `memory_store` / `memory_feedback` in the MCP server | `tm-mcp` | `usage.json` grows while driving Claude Code |
| 0.3 | Build `tracemind share-usage` — prints the JSON for a partner to paste back | `tm-cli` | round-trips end to end |
| 0.4 | Add `session_count` (a session = a contiguous run of activity, ≥30 min gap starts a new one) | shared crate | distinguishes 15 sessions from 15 queries in one afternoon |
| 0.5 | Landing page live with working email capture | `docs/landing/` | a stranger can reach you |

> 0.4 matters more than it looks. The exit gate is *sessions*, not queries. Without
> it you cannot tell one enthusiastic evening from two weeks of real use — which
> is the entire distinction a YC partner is asking about.

**Explicitly not in Week 0:** no new retrieval work, no new MCP verbs, no deck,
no desktop app. Feature freeze starts now and holds for ten weeks.

---

## 1. What gets measured

Local-only, opt-in, user-inspectable — consistent with the privacy claim. A
partner can `cat ~/.tracemind/usage.json` and see everything you'd ever see.

**Primary (the YC answer):**
- **Day-14 active** — partner has ≥1 session on a day ≥14 days after `first_seen`. This is *the* number.
- **Sessions per partner** — gate is ≥15.
- **Partners retained** — of N onboarded, how many are day-14 active.

**Secondary (the wedge answer):**
- **Retraction beats fired** — how often supersession triggers on real data. If this is ~0, the differentiator doesn't occur naturally and the pitch needs rethinking. **This is the single most important thing you will learn.**
- **Retraction beats acted on** — reconcile prompts resolved vs. ignored. Fires-but-ignored is a *worse* result than never firing, and you need to know.
- **`memory_feedback` calls** — currently lifetime zero. First real one is quotable.
- **`memory_context_for` invocations** — is the composition wedge actually used?

**Qualitative — do not skip:**
- What they tried to do that TraceMind couldn't.
- The moment (if any) it surprised them.
- Why they stopped, for anyone who does.

---

## 2. Recruiting — 5 partners

**Profile:** uses Claude Code or Cursor daily, comfortable with a CLI, on macOS,
and will tell you when something is bad. Existing relationship strongly
preferred — cold users won't give you the qualitative half.

**Sourcing, in order:** your own network first; MCP/Claude Code communities
second; the landing page last (slowest, least qualified).

**The ask, verbatim-ish:**

> I've built a local memory layer for Claude Code. It's early and almost nobody
> has used it. I'm looking for 5 people to run it for two weeks and tell me
> where it breaks. ~20 min to set up, one 15-min call a week. Everything stays
> on your machine — I only see what you choose to paste me.

**Target: 8 onboarded to land 5 active.** Attrition is normal; over-recruit.

**Onboarding must be ≤20 minutes.** Time it. If it isn't, fix that before
partner #2 — a bad install is the most common reason this whole plan fails, and
it fails silently because people just quietly stop.

---

## 3. Timeline

**Weeks 1–2 — instrument and onboard.**
Week 0 items land. Onboard partners 1–2, watch them install **over a call**.
Do not skip watching; the install is where you learn the most and it's the one
observation you can't recover later. Onboard the rest by end of week 2.

**Weeks 3–6 — the retraction beat in front of real people.**
All 5 running. Weekly 15-min calls. Target ≥15 sessions each by week 6.
The question under everything: *does supersession fire on real data, and does
anyone care when it does?*

**Weeks 7–8 — the number.**
First partners hit day 14 around week 4; all by week 6. Weeks 7–8 are for the
second cohort and for fixing whatever weeks 3–6 exposed.
**Decision point at end of week 8** — see §5.

**Weeks 9–10 — write.**
Application (draft exists), deck rebuild (audit exists), founder video, demo
video (script exists in the application draft). Everything is pre-written; these
two weeks are for replacing placeholders with real numbers.

---

## 4. Weekly check-in — 15 minutes, same five questions

Same questions every week; the *change* over ten weeks is the signal.

1. What did you use it for this week?
2. What did you expect it to do that it didn't?
3. Did it ever tell you it changed its mind? Did you care?
4. If I turned it off tomorrow, what would you miss? *(the Sean Ellis question — the one that actually predicts retention)*
5. What would make you tell someone else about it?

Write up the same day, in one file per partner under `docs/partners/`. Ten
weeks of these *is* the answer to YC's favourite question — "what have you
learned from users?" — and it can't be reconstructed later from memory.

---

## 5. Exit criteria (end of week 8)

| Outcome | Signal | What it means |
|---|---|---|
| **Strong** | ≥3 partners day-14 active, retraction beat fires and gets acted on | Apply W2027 with a real story. Lead with the beat. |
| **Mixed** | ≥3 active, but the beat rarely fires or is ignored | Retention is real, the *stated wedge* isn't. Re-derive the wedge from what they actually use, then apply. |
| **Weak** | <3 day-14 active | Do not apply. The product isn't retaining, and a YC batch won't fix that. Find out why first. |

The Mixed row is the most likely outcome and the most useful. Plan for it
rather than being surprised by it.

---

## 6. What this is *not*

- **Not a growth plan.** Five people. Not fifty. Depth over count.
- **Not a feature plan.** Ship nothing new for ten weeks unless a partner is blocked. The instinct that produced 101k lines against zero users is the exact instinct to suppress.
- **Not a metrics-theatre plan.** One number matters: day-14 active. Everything else is diagnosis.

---

## 7. First three things, in order

1. **Wire `usage.json` into `tm-mcp`** (§0.1–0.2). Until this lands, every week of partner usage is unrecoverable.
2. **Build `tracemind share-usage`** (§0.3). Without it partners have no way to send you anything.
3. **Message five people today.** Recruiting is the long pole and it runs in parallel with the code — don't serialize them.
