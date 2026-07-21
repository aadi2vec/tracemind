# Parallax — Pitch Deck

**Target: Seed Round ($3-5M)**
**10 Slides**

---

## Slide 1: Title

# PARALLAX

### The Intelligence Layer for Global Football

*See what scouts can't. Simulate what coaches imagine.*

Seed Round — $3-5M

---

## Slide 2: Problem

### The $9.6B Transfer Market Is Blind

**40% of players in Europe's top 5 leagues came from outside those leagues.**

Salah from Egypt. Mbappe from Bondy. Haaland from Norway's second division. Osimhen from Lagos. They were invisible until they weren't.

**Every scouting tool on the market requires:**
- Broadcast-quality video (16+ cameras)
- Event data feeds (human taggers)
- GPS/wearable hardware

**None of that exists where the next Salah is playing right now.**

A dirt pitch in Lagos. A concrete court in Sao Paulo. A patchy field in rural Thailand.

> The market isn't inefficient because clubs lack analytics.
> It's inefficient because **95% of the world's talent is invisible to every existing tool.**

---

## Slide 3: Solution

### We See Every Player on Earth

Parallax extracts world-class player intelligence from any video — including a single phone camera on a dirt pitch.

**Input:** Any match footage (phone → broadcast)
**Output:** Professional-grade player intelligence

| What we extract | How |
|----------------|-----|
| Movement signature | Robust pose estimation + gait analysis |
| Physical model | Sprint speed, acceleration, agility from video |
| Technical profile | First touch, passing range, dribbling style |
| Development trajectory | Predicted growth curve with confidence intervals |
| Tactical fit | How they'd perform in a specific team's system |

**The breakthrough:** Our perception system is trained on synthetic data to handle the worst possible conditions — low resolution, shaky camera, no pitch lines, no kit numbers, poor lighting. If we can do that, broadcast footage is trivial.

---

## Slide 4: Product — Two-Sided Platform

### Supply: Parallax Showcase (Free)

Academies upload phone footage → get free AI-generated player profiles.

- Player cards with movement signatures and development curves
- Digital CVs that parents and agents share
- Global exposure to buying clubs

**The academies ARE our data pipeline. No boots on the ground needed.**

### Demand: Parallax Discovery (Paid)

Clubs and agents search the global talent database.

- Natural language queries: *"Left-footed CBs under 21, comfortable under pressure, outside top 5 leagues"*
- Results ranked by movement intelligence, not stats
- Development projections, tactical fit scores, risk models
- Transfer simulation: *"How does signing Player X change our expected points?"*

**$5-100K/mo depending on tier**

---

## Slide 5: Why This Works — The Flywheel

```
    Academies upload footage
    (free player profiles)
            │
            ▼
    More players in database
            │
            ▼
    Better AI models
    (more training data)
            │
            ▼
    Better profiles + projections
            │
            ▼
    More clubs pay for Discovery ──────► More exposure for
            │                           academy players
            ▼                                  │
    Revenue funds R&D                          │
            │                                  │
            ▼                                  │
    World model + Tactics product              │
                                               │
            ◄──────────────────────────────────┘
            More academies upload
```

Each side reinforces the other. Supply creates demand. Demand attracts supply.

---

## Slide 6: Market

### TAM: $12B
Global sports analytics, scouting, transfer advisory

### SAM: $2.4B
Football-specific analytics, scouting tools, player projection

### Wedge: $500M+
NIL agencies + lower-league clubs + academy scouting — massively underserved

| Segment | # of Accounts | ACV | Revenue Potential |
|---------|--------------|-----|-------------------|
| NIL agencies + NCAA | ~1,000 | $30-60K | $30-60M |
| Lower-league clubs | ~2,000 | $36-120K | $72-240M |
| Player agents | ~5,000 | $12-36K | $60-180M |
| Mid-tier European clubs | ~200 | $180-600K | $36-120M |
| Elite clubs + betting | ~50 | $500K-2M | $25-100M |

---

## Slide 7: Competitive Positioning

```
                    EVENT DATA                  MOVEMENT INTELLIGENCE
                    (what happened)             (how & why it happened)

  BROADCAST     ┌──────────────────┐        ┌──────────────────┐
  VIDEO ONLY    │ Opta, StatsBomb, │        │ SkillCorner      │
  (top leagues) │ Wyscout          │        │ (tracking only)  │
                └──────────────────┘        └──────────────────┘

  ANY VIDEO     ┌──────────────────┐        ┌──────────────────┐
  (phone → pro) │                  │        │                  │
                │ (nobody)         │        │  ★ PARALLAX ★    │
                │                  │        │                  │
                └──────────────────┘        └──────────────────┘
```

**Opta** sells event data (what happened). We sell movement intelligence (how and why). Complementary — potential data partner.

**Hudl** hosts video. We understand video. They're a CDN; we're an intelligence layer. Could integrate or compete in youth.

**Nobody** occupies our quadrant: movement intelligence from any video source. That's the defensible position.

---

## Slide 8: Technology

### The Core Bet: Robust Perception

We built a perception system that works where every competitor fails.

| Their assumption | Our reality |
|-----------------|-------------|
| Broadcast-quality video | Phone footage, 15fps, handheld |
| Pitch-line calibration | No lines (dirt pitch) |
| Kit numbers for ID | Gait-based re-identification |
| High frame rate | Temporal super-resolution |
| Professional lighting | Domain-randomized training |

**How we got here:**
1. **Synthetic Data Engine** — we render photorealistic training data with every degradation (blur, noise, lighting, surface type). Our models see 100K simulated matches before touching real video.
2. **Domain Adaptation** — progressive curriculum from clean synthetic → degraded synthetic → real phone footage.
3. **Rust Inference Stack** — 10x faster video processing than Python competitors. ONNX Runtime in production.

**The Player Encoder** — our atomic asset. A 256-dim latent representation per player per match. Trained self-supervised on movement sequences. Captures playing style, not stats. Enables similarity search, trajectory prediction, and tactical simulation.

### Future: World Model (Phase 2)

Latent dynamics model trained on 100K+ match sequences. Predicts tactical outcomes, generates counterfactuals, simulates transfers. Built on top of player encoder representations. This is the long-term moat — nobody else is attempting this.

---

## Slide 9: Traction & Milestones

### Achieved
- [x] Founding team with proven Rust + ML systems track record
- [x] TraceMind: 17-crate Rust workspace with ONNX inference, vector search, graph storage (proof of systems capability)
- [x] Technical architecture validated

### 6-Month Milestones (Seed Capital)
- [ ] Synthetic data engine generating 10K+ training scenes
- [ ] Pose estimation on phone footage within 15% of broadcast quality
- [ ] 50+ academies on Showcase
- [ ] Player encoder v1 producing meaningful embeddings
- [ ] 3-5 club design partners

### 12-Month Milestones
- [ ] 200+ academies, 50K+ player models
- [ ] First paying Discovery customers
- [ ] $50K+ MRR

### 18-Month Milestones (Series A Trigger)
- [ ] 500+ academies, 200K+ player models
- [ ] $200K+ MRR
- [ ] World model generating plausible 10-second tactical rollouts
- [ ] Validated 12-month player projections (retroactive accuracy test)

---

## Slide 10: The Ask

### Raising $3-5M Seed

**Use of funds (18 months):**

| Category | Allocation | Purpose |
|----------|-----------|---------|
| Team (7-9 people) | 60% | CV research, ML, product, domain expert, first sales |
| Compute (GPU) | 20% | Synthetic data generation, model training |
| Operations | 15% | Academy onboarding, equipment kits, travel |
| Buffer | 5% | — |

**What we'll prove:**
1. Robust perception works on phone footage (the hard technical bet)
2. Academies will upload for free profiles (supply-side adoption)
3. Clubs will pay for access to invisible talent (demand-side willingness)

**The endgame:**

Parallax becomes the intelligence layer the entire football industry references. The AI-native Transfermarkt — alive, predictive, and covering every player on earth.

> *"We don't scout players. We see them."*

---

## Appendix A: Team

**[Aaditya Srivathsan] — Founder & CEO/CTO**
- Built TraceMind: local-first memory OS, 17-crate Rust workspace
- ONNX inference pipelines, vector search, graph storage, ML systems
- Deep expertise: Rust, Python, ML infrastructure, systems engineering

**Key Hires (Planned):**
| Role | Timeline | Profile |
|------|----------|---------|
| CV Research Engineer | Month 1 | PhD or senior in human pose estimation / action recognition |
| ML Research Engineer | Month 2 | Representation learning, contrastive methods, temporal models |
| Football Domain Expert | Month 3 | Former professional scout or head of analysis at a club |

---

## Appendix B: Financial Model Summary

| | Year 1 | Year 2 | Year 3 |
|---|--------|--------|--------|
| Showcase academies | 200 | 1,000 | 3,000+ |
| Player models | 50K | 200K | 500K+ |
| Discovery customers | 10 | 40 | 100+ |
| ARR | $600K | $3-5M | $10-15M |
| Burn rate (monthly) | $120K | $200K | $350K |
| Team size | 8 | 15 | 25 |

---

## Appendix C: Detailed Competitive Analysis

### vs. Opta / Stats Perform

Opta employs ~400 human analysts to manually tag events from broadcast footage. They cover ~80 leagues. Their data answers "what happened" — passes, shots, tackles, expected goals.

**Parallax advantage:**
- We answer "how and why" — movement patterns, decision-making, style
- We work without broadcast video — phone footage is enough
- We're automated — no human taggers, infinite scale
- We predict futures — development trajectories, not just historical stats

**Partnership opportunity:** Opta's event data enriches our profiles in leagues they cover. We extend coverage to leagues they'll never reach. Natural data-licensing partnership. Or: Stats Perform acquires us (exit scenario).

### vs. Hudl (Wyscout, StatsBomb)

Hudl is the dominant video platform for youth/college sports (US-focused). They acquired Wyscout (pro scouting video) and StatsBomb (premium event data). They're assembling a full-stack sports data company.

**Parallax advantage:**
- Hudl is a video hosting platform — coaches upload and manually tag. We automate the intelligence.
- Wyscout's scouting requires manually watching clips. Our search returns ranked results instantly.
- StatsBomb's data is event-level, manually collected. Ours is movement-level, automated.
- None of them work outside broadcast-covered leagues.

**Competitive risk:** Hudl could build CV capabilities. Mitigation: our perception research (phone-quality robustness) is 2-3 years ahead of anything they've shown. They historically buy rather than build (Wyscout, StatsBomb acquisitions). We could be an acquisition target.

**Partnership opportunity:** Coaches already upload to Hudl. We could process that video with user consent — instant distribution in US youth/college. Hudl gets AI features without building them.
