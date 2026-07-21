# Parallax — Design Specification

**Date:** 2026-05-09
**Status:** Draft v1
**Author:** Aaditya Srivathsan

> **Related docs:**
> - [PITCH_DECK.md](PITCH_DECK.md) — 10-slide investor deck
> - [ENGINEERING.md](ENGINEERING.md) — Full technical architecture and roadmap

---

## 1. Company Thesis

Parallax is a computer vision research company disguised as a football intelligence platform.

**Core bet:** We solve the hardest unsolved problem in sports CV — extracting world-class player intelligence from a single phone camera on a dirt pitch. If we crack that, broadcast footage is trivial, and we see every player on earth.

**One-liner:** The intelligence layer for global football — from dirt pitches to the Champions League.

**Why now:**
1. Pose estimation hit real-time on commodity hardware (MediaPipe, ViTPose) — 2024-2025
2. Video foundation models (DINOv2, VideoMAE) enable feature extraction without labeled data
3. Transfer fees hit $9.6B in 2023 — clubs need edge, not more dashboards
4. NIL in US college sports created a $1.2B market with zero scouting infrastructure
5. 40% of top-5-league players came from outside those leagues — the talent is invisible, not absent

---

## 2. The Problem

Every scouting and analytics tool on the market assumes broadcast-quality input: multiple synced cameras, pitch-line calibration, high frame rate, clean kits, professional lighting.

That assumption excludes 95% of the world's football talent.

- An academy in Lagos has phone footage and nothing else
- A youth tournament in rural Brazil has no event data feed
- A Scandinavian 3rd-division match has one camera and no tracking
- An MLS Next showcase has basic broadcast but no StatsBomb-level tagging

The transfer market isn't inefficient because clubs lack analytics. It's inefficient because the existing tools are blind to where the talent actually is.

---

## 3. The Solution: Two-Sided Platform

Parallax is a two-sided marketplace. The supply side provides data. The demand side pays for intelligence.

### 3.1 Supply Side — Parallax Showcase (Free)

**Users:** Academies, lower-league clubs, youth programs, college programs

**What they get:**
- AI-generated player profiles from phone/broadcast footage
- Development tracking over time (trajectory curves)
- Global exposure — their players visible to buying clubs worldwide
- Digital player CVs that parents and agents can share

**What they provide:**
- Video footage (phone camera on a tripod is enough)
- This IS the data pipeline — no boots on the ground needed

**Why they participate:**
- Free professional-grade player profiles
- Exposure to clubs they'd never reach through traditional scouting
- Development insights they can't afford from any other tool
- Parents and players demand it once it exists (social pressure drives adoption)

### 3.2 Demand Side — Parallax Discovery (Paid)

**Users:** Mid/top-tier clubs, agents, NIL agencies, betting firms

**What they get:**
- Search the full global talent database with natural language queries
- Player projections — development curves with confidence intervals
- Tactical fit scores — how a player would perform in a specific system
- Risk models — injury probability, adaptation difficulty
- Transfer simulation — expected impact on team performance
- Comparison engine — latent-space similarity across 500K+ players
- Discovery alerts — "3 players in the Tanzanian Premier League match your target profile"

**Pricing:**
| Tier | Price | Includes |
|------|-------|----------|
| Pro | $5-15K/mo | Search, player cards, alerts, 50 deep reports/mo |
| Club | $15-50K/mo | Unlimited reports, projections, tactical fit, API |
| Enterprise | $100-500K/yr | Custom models, transfer simulation, dedicated support |

### 3.3 The Flywheel

```
Academies upload footage (free profiles)
        ↓
More players in the database
        ↓
Better encoder (more training data)
        ↓
Better profiles + projections
        ↓
More buying clubs pay for Discovery
        ↓
More exposure for academy players
        ↓
More academies upload footage
        ↓
(repeat)
```

---

## 4. Competitive Landscape

### 4.1 Parallax vs. Opta (Stats Perform)

**Opta at a glance:** Owned by Vista Equity Partners (merged STATS LLC + Perform Group in 2019). Estimated $400-600M annual revenue. ~400 human analysts manually tag events from broadcast footage. Cover ~80 leagues, 40K+ matches/year, 20+ sports. Biggest revenue from betting (Bet365, DraftKings, Flutter). Also serve broadcasters (ESPN, Sky Sports), clubs (Centre Circle scouting tool), and gaming (EA Sports FC player ratings).

**Key products:** Event data feeds (API), xG/xT models, AutoStats (broadcast-derived CV tracking at 25fps), RunningBall (ultra-low-latency betting feeds), PressBox (broadcaster graphics), Centre Circle (club scouting/recruitment).

| Dimension | Opta / Stats Perform | Parallax |
|-----------|---------------------|----------|
| **What they sell** | Event data (passes, shots, tackles) + packaged analytics (xG, player ratings). Also AutoStats broadcast-derived tracking. | Movement intelligence from video — latent player representations, trajectory predictions, tactical simulation |
| **Data collection** | ~400 human analysts manually tag 1,500-2,000 events per match from broadcast footage. Multi-pass QA. Proprietary taxonomy (~70+ event types). | Automated CV pipeline. Zero human taggers. Works on broadcast AND phone footage. |
| **Coverage gap** | Zero coverage below tier 2-3. No youth, no academies, no lower leagues without broadcast deals. Human-tagger model is expensive and slow to scale to new leagues. | Designed for zero-infrastructure markets. Phone footage is the minimum viable input. Automated = infinite scale. |
| **AI/ML** | xG, AutoStats (broadcast CV tracking), win probability, NLG content automation. AutoStats is their biggest AI investment but still broadcast-quality-only. | Computer vision from degraded video, pose estimation, gait Re-ID, learned player representations, world models — fundamentally different and more ambitious ML stack |
| **Weaknesses we exploit** | Expensive/rigid enterprise contracts (no dev tier, no self-serve). Walled garden. AutoStats requires broadcast quality. Human taggers don't scale. Vista PE ownership may limit R&D spend. | — |
| **Relationship** | **Partner, not competitor.** Complementary data layers. Opta's event data enriches Parallax profiles. Parallax extends into markets Opta will never reach (human taggers can't scale there). Potential data licensing or acquisition target (Stats Perform could acquire Parallax to get the tracking layer they lack). |

**Key insight:** Opta answers "what happened" (event-level). Parallax answers "how and why" (movement-level). These are complementary. Opta's structural vulnerability is the human-tagger model — if Parallax achieves comparable event detection automatically, we undercut their economics entirely.

### 4.2 Parallax vs. Hudl (incl. Wyscout, StatsBomb, InStat)

**Hudl at a glance:** Founded 2006 in Lincoln, Nebraska. ~6 million athletes, 200K+ teams. Raised $100M+ (Nelnet, Accel). Valued at $1-3B (reported). Dominates US high school and college video. Aggressive M&A: Wyscout (2019), InStat (~2022), StatsBomb (2024). Also acquired Realtrack/Wimu (GPS wearables) and Volleymetrics.

**Key products:**
- **Core Hudl**: Video upload, tagging, sharing, highlight reels. US youth/HS/college.
- **Hudl Focus**: Automated cameras (hardware). AI-controlled PTZ, no operator needed. Deployed in HS/college venues.
- **Hudl Assist**: Human-powered video tagging service (~$50-150/game). Offshore taggers.
- **Hudl Sportscode**: Pro-grade desktop video analysis (macOS). Live coding. Used by NFL, NBA, Premier League.
- **Wyscout**: Pro football scouting database — 550K+ players, 2,000+ competitions. The "LinkedIn of football scouting."
- **StatsBomb**: Premium event data, xG, 360 freeze-frame data. $50-500K+/yr enterprise licensing.
- **InStat**: Match analysis + scouting for pro football (strong in Russia/Eastern Europe).

| Dimension | Hudl / Wyscout / StatsBomb | Parallax |
|-----------|---------------------------|----------|
| **What they sell** | Video platform + scouting database + event data. Three separate products, still being integrated post-acquisition. | AI-native intelligence — the analysis IS the product, not the video |
| **Video model** | Coaches upload video, manually tag (or pay $50-150/game for Assist human tagging). Wyscout: human-curated clip library. Video hosting is the product. | Video is the INPUT, intelligence is the OUTPUT. No manual tagging. Full automation. |
| **Scouting workflow** | Wyscout: scout searches by position/league, watches clips, makes subjective assessment. 550K players but all from broadcast, all manually tagged. | Natural language queries → ranked results with movement-based projections and fit scores |
| **Youth/amateur** | Dominates US youth/HS/college (170K+ teams). But it's video hosting — no AI intelligence layer. Highlight reels for recruiting. Hudl Focus cameras in venues. | Parallax Showcase: free AI-generated profiles from phone footage. Transforms passive video into active intelligence. |
| **AI/ML capability** | Auto play detection (American football snap-to-whistle). Focus camera auto-PTZ. StatsBomb 360 freeze-frames. **But**: much of tagging is still human-powered. No automated tactical analysis. No NL querying. No player tracking from broadcast at scale. | CV-first company. Every capability is automated: pose estimation, tracking, gait Re-ID, player encoding, event detection. |
| **Key weaknesses** | Product fragmentation post-acquisitions (Wyscout ≠ StatsBomb ≠ Sportscode). Legacy tech (Sportscode is aging macOS desktop app). AI lags behind marketing. US-centric DNA. **Critical gap: no real-time tracking layer** — Second Spectrum, Hawk-Eye, Stats Perform own this. | — |
| **Antitrust risk** | Wyscout + InStat + StatsBomb acquisitions raise monopoly concerns in pro football data. Regulators may intervene. | Opens space for independent alternative |
| **Relationship** | **Competitor AND partner.** Near-term: integrate with Hudl (coaches already upload there → Parallax processes their video). Hudl gets AI features they lack; Parallax gets distribution into 200K+ teams. Long-term: Parallax Showcase competes directly with Hudl in football. Hudl historically acquires rather than builds — we could be an acquisition target. |

**Strategic positioning vs. Hudl:**
- Hudl is assembling pieces via M&A. Integration is slow and products remain fragmented.
- Parallax is purpose-built from scratch — unified architecture, AI-native, no legacy.
- Hudl's moat is network effects and breadth (6M athletes). Our moat is perception depth (phone-footage robustness nobody else has).
- **The critical gap Hudl cannot fill**: they don't own the tracking layer. Second Spectrum (Genius Sports), Hawk-Eye (Sony), and Stats Perform own optical tracking. Parallax IS a tracking layer — derived from any video, including phone footage. This is the wedge.

### 4.3 Parallax vs. Other Competitors

| Competitor | What they do | Revenue/Scale | Parallax advantage |
|------------|-------------|--------------|-------------------|
| **SciSports** | Player ratings, recruitment tool (SciSkill model) | Small, Netherlands-based | Dashboard on public data. No video understanding, no world model, no invisible-market coverage |
| **SkillCorner** | Physical/tracking data from broadcast video | Growing, VC-backed | Tracking only — no latent representations, no projection, no simulation. Requires broadcast quality. Closest to what we do but limited to broadcast. |
| **Second Spectrum** (Genius Sports) | Real-time optical tracking, NBA/MLS/Premier League | Acquired for ~$200M | Best-in-class tracking but requires in-stadium hardware or high-quality broadcast. Can't work on phone footage. |
| **Hawk-Eye** (Sony) | Ball tracking, goal-line tech, optical player tracking | Owned by Sony | Hardware-dependent. Top-tier leagues only. |
| **Metrica Sports** | Tracking data provider | Small, Spain-based | Raw data provider, not intelligence. Could be a Parallax data source |
| **21st Club** | Strategic consultancy + analytics | Small, acquired by City Football Group | Consulting model, doesn't scale. No proprietary perception |
| **Catapult/STATSports** | GPS wearable tracking | Catapult: ~$100M revenue | Hardware-dependent, requires players to wear devices. No video, no scouting at scale |
| **Genius Sports** | Data + tracking for betting/media | Public, ~$500M revenue | NFL-focused, betting-centric. Strong in US but less in global football scouting |

### 4.4 Competitive Summary

```
                        EVENT DATA                    MOVEMENT INTELLIGENCE
                        (what happened)               (how & why)
                    ┌─────────────────────┐       ┌─────────────────────┐
  BROADCAST/STADIUM │  Opta, StatsBomb,   │       │  SkillCorner,       │
  ONLY              │  Wyscout, Genius    │       │  Second Spectrum,   │
  (top leagues)     │                     │       │  Hawk-Eye           │
                    └─────────────────────┘       └─────────────────────┘
                    ┌─────────────────────┐       ┌─────────────────────┐
  ANY VIDEO         │                     │       │                     │
  (phone → pro)     │  (nobody)           │       │  ★ PARALLAX ★       │
                    │                     │       │                     │
                    └─────────────────────┘       └─────────────────────┘
```

Parallax occupies the only quadrant that matters for the future: movement intelligence from any video source. Every other player is locked to broadcast/stadium infrastructure.

### 4.5 Partnership Strategy

| Partner | Integration | Value Exchange |
|---------|------------|----------------|
| **Opta / Stats Perform** | Fuse event data into Parallax profiles for covered leagues | Opta gets extended coverage into invisible markets; Parallax gets richer profiles. Stats Perform could acquire Parallax to fill their tracking gap vs. Genius Sports. |
| **Hudl** | Process video already uploaded to Hudl (200K+ teams). Coaches opt in to Parallax analysis. | Hudl gets the AI layer they've failed to build internally; Parallax gets instant distribution. Risk: Hudl may try to acquire or copy. |
| **Catapult / STATSports** | Fuse GPS/wearable data with video-derived movement data | Physical data enriches profiles; Parallax provides tactical context wearables can't capture |
| **Transfermarkt** | Display Parallax player profiles, computed valuations | Transfermarkt modernizes; Parallax becomes the canonical reference layer |
| **FBref / Football Reference** | Embed Parallax movement signatures alongside traditional stats | Academic credibility; user acquisition |
| **Genius Sports** | Parallax extends their tracking to non-stadium settings | Genius gets coverage beyond their hardware-dependent systems; potential strategic acquirer |

### 4.6 Exit Scenarios

| Acquirer | Rationale | Estimated Range |
|----------|-----------|----------------|
| **Hudl** | Fills their AI/tracking gap. Extends Wyscout into invisible markets. | $50-200M (acqui-hire to strategic) |
| **Stats Perform** | Competes with Genius Sports' tracking. Extends coverage beyond human taggers. | $100-300M |
| **Genius Sports** | Adds phone-footage tracking to complement stadium hardware. | $100-500M |
| **Sony (Hawk-Eye)** | Software layer on top of their hardware tracking. | $100-300M |
| **IPO** | If $50M+ ARR, clear path to public as AI-native sports intelligence. | $500M-1B+ |

---

## 5. Product Detail

### 5.1 Parallax Showcase (Supply Side — Free)

**Mobile capture app:**
- Dead-simple: open app → point at pitch → record
- Works on any phone (iOS/Android), no special hardware
- Auto-uploads when on WiFi
- Minimal metadata input: team names, player names/numbers (optional — gait Re-ID handles the rest)

**Player profile (auto-generated from video):**
- Movement signature visualization (radar chart of movement qualities)
- Physical model: sprint speed, acceleration, deceleration, agility score
- Technical indicators: first touch quality, passing range, dribbling style
- Decision speed: time between receiving ball and action
- Positional heatmap: where they actually play vs. nominal position
- Development curve: trajectory over time (requires 3+ matches)
- Comparable players: "movement profile most similar to..."
- Shareable link / embeddable card

**Academy dashboard:**
- All players, development over time
- Team-level insights (pressing intensity, possession style)
- Exportable reports for parents, agents, clubs

### 5.2 Parallax Discovery (Demand Side — Paid)

**Search engine:**
- Natural language: "left-footed CBs under 21 comfortable progressing under pressure, outside top 5 leagues, contract expiring within 18 months"
- Filter by movement qualities, not just stats
- Results ranked by embedding similarity to ideal profile
- Each result includes: player card, development curve, tactical fit score, risk assessment

**Projection engine:**
- Development curve prediction (6/12/24 month horizon)
- Confidence intervals based on comparable player trajectories
- "This player profiles like a young Modric at the same age, with faster lateral acceleration but lower passing range"

**Tactical fit:**
- Input: target team's system (formation, pressing style, build-up pattern)
- Output: fit score + explanation ("high fit for possession-based 4-3-3, risk in transition defense")
- Powered by the world model once it matures

**Transfer simulator:**
- "If we sign Player X, how does our expected performance change?"
- Factors in: player profile, team system, league difficulty, adaptation curve

**Recruitment workflow:**
- Shortlisting → scouting assignment → evaluation → decision pipeline
- Collaborative (multiple scouts, analysts, sporting directors)
- Audit trail for every recommendation

### 5.3 Parallax Tactics (Phase 2 — World Model)

**Formation simulator:**
- Input: 22 players + formation + tactical instructions
- Output: simulated match dynamics (pitch control, passing probability, pressing effectiveness)
- Monte Carlo rollouts through the learned world model

**Counterfactual engine:**
- "What would have happened if Saka cut inside instead of crossing?"
- Re-simulates from a decision point with an alternative action

**Game prep:**
- Opponent analysis: tendencies, exploitable patterns, set-piece vulnerability
- Automated report generation

---

## 6. Business Model

### 6.1 Revenue Streams

| Stream | Price | Target | Timeline |
|--------|-------|--------|----------|
| **Discovery SaaS** (Pro) | $5-15K/mo | Agents, small clubs, NIL agencies | Month 10+ |
| **Discovery SaaS** (Club) | $15-50K/mo | Mid-tier clubs | Month 12+ |
| **Discovery SaaS** (Enterprise) | $100-500K/yr | Top clubs, betting firms | Month 18+ |
| **Tactics SaaS** | $30-100K/mo | Clubs, broadcasters | Month 20+ |
| **API access** | Usage-based | Betting, gaming, media | Month 14+ |
| **Transfer advisory** | 1-3% success fee | On deals sourced via Parallax | Month 18+ |
| **Data licensing** | Custom | Broadcasters, gaming companies | Month 16+ |

### 6.2 Unit Economics Target

| Metric | Target |
|--------|--------|
| CAC | $5-15K (direct sales) |
| ACV (blended) | $50-120K |
| LTV/CAC | >5x |
| Gross margin | 75%+ (compute is main COGS) |
| Payback period | <12 months |

### 6.3 Financial Projections

| Milestone | Timeline | Revenue |
|-----------|----------|---------|
| First pilot | Month 9-10 | $0 (free pilots) |
| First revenue | Month 12 | ~$10K MRR |
| Product-market fit | Month 16 | $50-100K MRR |
| Series A trigger | Month 18-20 | $200K+ MRR |
| Scale | Month 30 | $500K-1M MRR |

---

## 7. Go-to-Market

### 7.1 Supply Side (Showcase — Free)

**Phase 1: Seed academies (Month 6-10)**
- Partner with 20-50 academies in target markets (Nigeria, Ghana, Senegal, Brazil, US youth)
- Direct outreach to academy directors — "free AI scouting profiles for your players"
- Provide cheap phone tripod kits ($20 each) for recording quality

**Phase 2: Viral growth (Month 10-16)**
- Parents share player profiles on social media
- Academies recruit by showing Parallax profiles to prospects
- Content: "Parallax Top 50 Under-18 in West Africa" — monthly discovery lists
- Academy referral program: invite 3 academies → unlock premium development analytics

**Phase 3: Platform pull (Month 16+)**
- Clubs on the Discovery side request specific leagues/academies
- Parallax reaches out to those academies to onboard them
- Demand-driven supply expansion

### 7.2 Demand Side (Discovery — Paid)

**Phase 1: Design partners (Month 8-12)**
- 3-5 clubs as unpaid design partners (lower-league European + MLS)
- They shape the product; they get free access during beta
- Target: clubs with known analytical cultures (Brentford model, Brighton model)

**Phase 2: Direct sales (Month 12-18)**
- Hire 1 sales rep with football network
- Target: sporting directors, heads of recruitment
- Wedge: "we have coverage of X players in Y leagues that nobody else does"

**Phase 3: Enterprise (Month 18+)**
- Top clubs, betting firms, broadcasters
- Custom integrations, dedicated support
- Transfer advisory (success-fee model)

---

## 8. Team Plan

| Role | When | Why | Comp |
|------|------|-----|------|
| **Founder/CEO** (you) | Day 0 | Rust infra, ML, systems, vision | Founder equity |
| **CV Research Engineer** | Month 1 | Pose estimation, tracking, synthetic data | $150-200K + equity |
| **ML Research Engineer** | Month 2 | Player encoder, world model | $150-200K + equity |
| **Football Domain Expert** | Month 3 | Former scout/analyst. Validates outputs, shapes product | $80-120K + equity |
| **Product/Design** | Month 6 | Dashboard, mobile app, UX | $130-170K + equity |
| **Sales (football network)** | Month 8 | Someone who knows sporting directors | $100-140K + commission + equity |
| **2x ML Engineers** | Month 10 | Scale training, perception robustness | $140-180K + equity |
| **Mobile Engineer** | Month 10 | Capture app (iOS/Android) | $140-180K + equity |
| **Data Engineer** | Month 14 | Pipeline reliability at scale | $140-180K + equity |

**First 18 months: 7-9 people. Burn: $100-150K/mo.**

---

## 9. Fundraising Strategy

### 9.1 Pre-Seed: $1-1.5M (Month 0-2)

**Source:** Angels, small funds, football-connected investors

**Pitch:** "We're solving the hardest unsolved problem in sports CV. Here's our synthetic data approach and our founding team."

**Use of funds:**
- 12 months runway for 3-person team
- GPU compute for synthetic data + model training
- 20 phone tripod kits for seed academies

**Milestones to hit:**
- Synthetic data engine v1
- Pose estimation working on phone footage
- 5 academy partnerships signed

### 9.2 Seed: $3-5M (Month 6-8)

**Source:** Sports-tech VCs + ML-native funds

**Targets:**
- Elysian Park Ventures (Dodgers ownership, sports-tech)
- Courtside Ventures (sports-tech, European football focus)
- SeventySix Capital (sports innovation)
- Lux Capital (deep-tech)
- Radical Ventures (ML-first)
- Conviction (applied AI)

**Pitch:** "We proved robust perception works on phone footage. Here's the demo. Now we're building the player encoder and shipping to academies."

**Demo:** Side-by-side comparison — broadcast footage analysis vs. phone footage analysis, showing near-equivalent output quality.

**Use of funds:**
- 18 months runway for 7-9 person team
- Scale GPU compute
- 200+ academy onboarding
- First sales hire

### 9.3 Series A: $12-18M (Month 18-22)

**Source:** Growth-stage VCs

**Pitch:** "We have X paying customers, X players in our database, coverage of markets nobody else touches. We're the AI-native Transfermarkt. Now we're building the world model."

**Triggers:**
- $200K+ MRR
- 500+ academies on Showcase
- 100K+ player models in database
- Validated projections (show that our 12-month-ago predictions were accurate)

---

## 10. Risk Analysis

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Robust perception doesn't work well enough on phone footage** | Critical | Synthetic data engine de-risks this. Progressive quality tiers — even 70% of broadcast quality is 100x better than nothing in invisible markets |
| **Academies don't upload** | High | The incentive is real (free profiles, exposure). Seed with 20 academies, prove adoption, then scale. Worst case: hire local ambassadors ($500/mo) to manage 50 academies each |
| **Player encoder doesn't produce meaningful representations** | High | Start with proven approaches (contrastive learning on pose sequences). Validate with domain experts early. Fall back to explicit feature engineering if learned representations fail |
| **Hudl copies the approach** | Medium | Hudl is a video platform, not an ML research company. Their acquisition of StatsBomb suggests they buy rather than build. They may be an acquirer, not a competitor |
| **Data privacy (GDPR, player consent)** | Medium | Showcase requires player/guardian consent during upload. Clear data usage policy. Option to delete profiles. No data sold without consent |
| **World model doesn't produce useful simulations** | Medium | World model is Phase 2. Scout product stands alone without it. World model is upside, not dependency |
| **Market timing — clubs resist AI scouting** | Low | Clubs already use analytics. This is better analytics, not a paradigm shift. The paradigm shift is coverage, not methodology |

---

## 11. Three-Year Arc

**Year 1: Prove the perception and build the supply side**
- Robust phone-video perception pipeline
- Player encoder v1
- 200+ academies on Showcase
- First paying Discovery customers
- Target: $100K MRR by end of year

**Year 2: Become the reference layer**
- 1,000+ academies, 200K+ player models
- Discovery product mature (search, projections, fit scores)
- Tactics product v1 (world model)
- Content: monthly discovery lists, breakout predictions
- Target: $500K-1M MRR, community of 500K+ monthly users

**Year 3: The new Transfermarkt**
- Global coverage — every league, every academy that wants in
- Enterprise customers (top clubs, betting, broadcast)
- Tactics simulator in production
- Transfer advisory generating success fees
- Target: $5-10M ARR, clear path to $50M

---

## 12. Why This Team, Why Now

**Why you:**
- Built TraceMind — a 17-crate Rust workspace with ONNX inference, vector search, graph storage, and ML pipelines. The exact stack Parallax needs.
- Deep ML background + systems engineering. Rare combination for sports CV.
- Football domain passion — this isn't a market you're entering for money, it's one you understand.

**Why now:**
- Pose estimation just became real-time on commodity hardware
- Video foundation models eliminate the need for massive labeled datasets
- The transfer market's inefficiency is becoming obvious (clubs that adopt analytics — Brentford, Brighton, Atalanta — consistently outperform their budgets)
- NIL in US college sports created a new market overnight
- No one is building for invisible markets because the perception problem is "too hard"

That's exactly why it's the right problem to solve.
