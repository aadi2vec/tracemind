# Investor deck audit — 2026-07-28

Both decks audited: `INVESTOR_DECK.pdf` and `TraceMind_Investor_Deck.pptx`.

**Headline: there is no integrity problem, but both decks are unusable.**
They are the same deck in two formats, both authored **2026-05-06/07** — one
full strategy pivot ago. The PDF's file mtime says Jul 21, but its *content* is
dated May 6; it was re-exported, not updated.

---

## 1. The good news

**The retired 75.49 F1 does not appear in either deck.** I checked with
Spotlight and by extracting the full text. The only F1 quoted is **25.70**,
which *understates* current reality (50.32 fresh / 70.49 held-out).

So the risk I flagged earlier — a deck circulating a number you publicly
retracted — **does not exist**. Every error runs in the conservative direction.
That is the right direction for errors to run.

---

## 2. The appendix points at nothing

This is the most damaging finding. The final slide is titled *"Where to verify
everything in here"* and lists four documents as **"four canonical, all
current."** All four are gone:

| Cited as canonical | Status |
|---|---|
| `docs/PHASE4_DELIGHT.md` | **missing** |
| `docs/INTENT_SYSTEM.md` | **missing** |
| `docs/BRAIN_ARCHITECTURE.md` | **missing** |
| `docs/LOCOMO_RESULTS.md` | **missing** |
| `demo/recordings/walkthrough-clean-20260506-215825.mov` (the "6-minute proof") | **missing** |

If anyone took the deck up on its offer to verify, every link is dead. For a
deck whose entire pitch is *"be honest, not polished,"* a verification section
that cannot verify is the worst possible failure mode.

---

## 3. Every quantitative claim is stale — all understating

| Claim | Deck (May 6) | Reality (Jul 28) | Direction |
|---|---|---|---|
| Rust crates | 17 | **30** | understates |
| Lines of code | ~12k | **101,501** | understates by ~8× |
| Tests passing | 163 | **1,240** | understates |
| MCP tools | 21 | 65 tool strings; core-6 surface | understates |
| LoCoMo F1 | 25.70 (mini, BGE) | **50.32** fresh / 70.49 held-out | understates |

The deck describes a substantially smaller, weaker product than the one that
exists. Slide 7 ("What's bad") even frames F1 25.7 as the central weakness and
blames Tier-0 extraction — an analysis you have since superseded by actually
fixing retrieval.

---

## 4. The strategy is a pivot out of date

The deck sells **"a system of intents"** with the `Commitment` primitive as the
wedge, and **"brain-shaped architecture — 6 memory types × 10 cognitive ops"**
as the moat.

Per `CHARTER-H2-2026.md` and your own memory notes, both have been superseded:
the wedge is now the **composition layer** (`memory_context_for`) and the
**retraction beat**; the brain-shaped framing is not how you describe the
system today. The Sprint A–G plan on slide 9 is likewise replaced by the H2
charter.

Also stale: the competitor list (Mem0, Honcho, Letta, Zep, SuperLocalMemory)
omits **Supermemory**, which is closest to your positioning, and does not note
that **Mem0 is YC-backed** — the single most important competitive fact for a
YC audience.

---

## 5. Framing mismatch

Both decks are explicitly addressed *"For: a friend who's looking"* with
**"Status: Pre-seed, no raise yet, asking for thinking time + intros, not a
check"** and an ask of *"read this / watch the demo / two intros."*

That is a friend-review artifact, not a fundraising deck. Sending it to an
investor would read as either confused or as a soft-circle attempt.

---

## 6. What this means for the Fall 2026 decision

**It doesn't block it.** The YC application is a **form plus two videos** — it
does not take a deck. The deck's condition is therefore irrelevant to whether
you submit late this week.

The Fall decision still rests entirely on the thing that hasn't changed: the
honest answer to *"Are people using your product?"* is **no**. My
recommendation from the readiness memo is unchanged — **submit late,
timeboxed to one day, using `YC-APPLICATION-DRAFT-2026-07-28.md` as-is.**

**Do not spend any of that day on the deck.**

## 7. What to actually do with the decks

1. **Retire both now.** Move to `docs/archive/` or clearly mark them
   `SUPERSEDED — 2026-05-06, do not send`. The realistic risk is not that they
   lie; it's that you send one under time pressure and it describes a 12k-LOC
   project with a wedge you no longer pitch.
2. **Rebuild once, after the wedge sentence is chosen** (readiness memo §5).
   Rebuilding before that decision means rebuilding twice.
3. **When you rebuild:** the numbers only need updating, not spinning — they
   all move in your favour. Lead with the retraction beat, keep the "what's
   bad" slide (it is the best thing in the current deck), and make the appendix
   point at files that exist: `MVP-STATUS-2026-07.md`,
   `HOLISTIC-REVIEW-2026-07.md`, `CHARTER-H2-2026.md`, and the native GUI
   walkthrough in `docs/demos/gui/` once it's recorded.
4. **Timing:** needed for angel conversations and for W2027, not for the late
   Fall form. Roughly one day once the wedge is fixed.
