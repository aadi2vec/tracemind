# DP weekly check-in script (DP-4)

**Audience.** Every design partner gets one 15-minute call per week. No exceptions, no rescheduling beyond 24 hours. The script below is what we walk through.

**Goal.** Two artifacts per call: (a) a row in the DP scoreboard with quantitative usage, and (b) a free-text diary entry with the partner's verbatim language about what hurt this week. The verbatim language is what we mine for messaging and next-sprint scope.

**Stance.** Listen. Do not pitch. Do not defend. Every "this is wrong" is a gift — write it down word-for-word. Every "I uninstalled it because…" is a sprint plan.

**Privacy.** The partner's `usage.json` is theirs, not ours. We ask them to copy-paste it; we never pull it remotely. If they refuse, we move on with self-reported numbers — that refusal is itself useful data.

---

## Before the call (5 min prep)

- Open the partner's row in `dp-scoreboard.md` (one row per partner; columns at the bottom of this doc).
- Re-read last week's diary entry. Note any commitments *we* made.
- Open a blank diary entry for this week dated `YYYY-MM-DD`.

---

## During the call (15 min, 6 questions)

Ask exactly these six questions in order. Read them verbatim — wording matters because we are comparing across partners.

### 1. What did you ask Claude / Goose / Cursor / Cline this week? (3 min)

Free-form. Let them list. Capture every prompt category they mention:
- code Q&A
- debugging
- writing prose / email
- "what did I decide about…" / persistence
- multi-project / context switches
- agentic task (PR, refactor, deploy)
- anything else

If they say "the usual" — push: "give me three examples from yesterday."

### 2. Of those, where did TraceMind change the answer? (3 min)

Specifically:
- Did Claude/Goose call `memory_query` or `memory_store`?
- Did the answer cite a fact you'd told it days/weeks ago?
- Did the contradiction beat fire? (Was there a moment where TraceMind said "you previously said X, are you sure?")
- Did you switch contexts? Did you notice anything *not* leaking that you'd expected to?

Capture each "yes" with one verbatim sentence from them.

### 3. Where did TraceMind get it wrong? (3 min)

Three buckets:
- **Bad recall** — it surfaced something irrelevant or stale.
- **Missed recall** — it should have known and didn't.
- **Bad capture** — it stored something it shouldn't have (PII, junk, draft text). This one is critical for trust.

For each, ask: "Did you give it feedback?" If `helpful` / `not_related` / `cross_context_bridge` — write down which.

### 4. What would make you uninstall the MCP this week? (2 min)

Read this one literally. Do not soften. If they say "nothing right now" — push: "what's the closest you got to uninstalling, even briefly?"

### 5. What did you wish it could do that it didn't? (2 min)

Wishlist. Capture verbatim. Do not commit. Do not say "that's on the roadmap." Just write it.

### 6. Want to share your usage.json? (1 min)

> "Settings → Capture sources → 'Copy usage JSON (for sharing with your DP contact)'. Paste it in our DM whenever — no rush."

If they say no, note it. Move on.

---

## After the call (5 min)

Update the partner's scoreboard row:

| Column | How to fill |
|---|---|
| `active_days_w2` | from their usage.json `active_days` array length (or self-reported number of days they opened a Claude/Goose session that called a `memory_*` tool) |
| `total_queries` | usage.json `total_queries` |
| `total_helpful` / `total_negative` | from usage.json — gives F-1 reinforcement signal |
| `contradictions_surfaced` | self-reported count of times the retraction beat fired this week |
| `uninstall_risk` | one of: none / latent / acute. "Acute" = they named a specific reason in Q4. |
| `top_wish` | the strongest wishlist item from Q5, verbatim |
| `top_failure` | the most painful Q3 item, verbatim |

Append the diary entry to `dp-diary/<partner-handle>.md` with the date, all six answers captured as bullets, and a one-line **so what?** at the end — your own read on what the week's data means.

---

## Aggregation (Friday, 30 min, across all partners)

Friday evening, walk the diary entries and produce one paragraph for the team Slack:

- **W2 retention gate (DP-5):** how many partners hit ≥ 3 active days this week? If < 3 of 5, raise the alarm — the wedge sentence is wrong, and DP-5 says stop building until we re-brainstorm.
- **Top three wishes across the cohort.** If two or more partners wished for the same thing, that's a candidate for the next sprint.
- **Top failure category.** Bad recall? Missed recall? Bad capture? UI? Plot the trend over weeks — the bucket that's *not* getting better is the bucket that becomes next sprint's P0.
- **Contradiction-beat firings.** If the moat hasn't fired in two consecutive weeks, the partners aren't deep enough — the wedge isn't sticking. Reconsider onboarding.
- **One direct partner quote** that ought to be on the next pitch deck.

---

## Scoreboard schema (`dp-scoreboard.md`)

```
| handle | start_date | host(s) | week | active_days_w2 | total_queries | total_helpful | total_negative | contradictions_surfaced | uninstall_risk | top_wish | top_failure |
|--------|-----------|---------|------|----------------|---------------|---------------|----------------|--------------------------|----------------|----------|-------------|
```

One row per partner per week. Append-only. The DP-5 gate is computed from the most recent two rows per partner.

---

## What this script is *not*

- **Not a sales call.** No demo, no feature walkthrough, no pitch. They've already installed it; the question is whether it earned a second week.
- **Not a feature request intake.** Wishes are captured, not promised. The roadmap is decided weekly from the *aggregate* signal in `dp-scoreboard.md` plus the DP-5 retention gate — never from a single partner's call.
- **Not anonymous.** The diary entries are pseudonymous (`<partner-handle>.md`) but each partner knows their data informs the next sprint, by name. That's part of the deal we made in the consent step at sign-up.

---

## What done looks like

After four consecutive weeks of running this script:

- We have 20+ diary entries (5 partners × 4 weeks) with verbatim language.
- We have a 20-row scoreboard with `usage.json`-grounded retention numbers.
- We have a ranked list of failure categories from real use.
- We know whether DP-5 (≥ 3 of 5 hit W2) is green, amber, or red — and therefore whether the seed deck's W2 retention claim is actually true.
- We have at least one partner ready to record DP-6.

If any of those is missing, the check-in didn't happen — it sat in someone's calendar. Run it again next week, properly.
