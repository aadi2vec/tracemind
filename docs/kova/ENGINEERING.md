# Parallax — Engineering Architecture

**Date:** 2026-05-09
**Status:** Draft v1

---

## 1. System Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                        APPLICATIONS                               │
│  Showcase App (Mobile)  │  Discovery Dashboard  │  API  │  Alerts │
├──────────────────────────────────────────────────────────────────┤
│                       INTELLIGENCE                                │
│  Player Encoder │ Trajectory Predictor │ Similarity Search        │
│  World Model │ Tactical Simulator │ Risk Model │ Fit Scorer       │
├──────────────────────────────────────────────────────────────────┤
│                    ROBUST PERCEPTION (the moat)                   │
│  Degraded Pose Est. │ Gait Re-ID │ Markerless Calibration        │
│  Temporal Super-Res │ Ball Detection │ Event Detection │ Tracking │
├──────────────────────────────────────────────────────────────────┤
│                    SYNTHETIC DATA ENGINE                           │
│  Scene Renderer │ Degradation Sim │ Motion Generator              │
│  Curriculum Scheduler │ Domain Randomization                      │
├──────────────────────────────────────────────────────────────────┤
│                        DATA LAYER                                 │
│  Video Store │ Feature Store │ Vector DB │ Graph │ Time-Series    │
│  Model Registry │ Job Queue │ Metrics                             │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. Language & Stack Decisions

| Layer | Language | Rationale |
|-------|----------|-----------|
| **Video pipeline + inference** | Rust | Performance-critical, zero-cost abstractions, ONNX Runtime bindings (`ort` crate), FFmpeg bindings. Founder's primary language. |
| **Model training** | Python (PyTorch) | Ecosystem — every CV model, every pretrained checkpoint, every training framework is Python-first. No way around this. |
| **Synthetic data engine** | Python + Unity/Unreal C# | Unity ML-Agents for scene generation. Python orchestration for curriculum and randomization. |
| **Mobile capture app** | Flutter or React Native | Cross-platform (iOS/Android), camera access, background upload. |
| **Discovery dashboard** | TypeScript (Next.js) or Tauri | Web-first for broad access. Tauri option for desktop (reuses Rust backend). |
| **API** | Rust (Axum) | Low-latency, type-safe, shares code with inference pipeline. |
| **Infrastructure** | Docker + Kubernetes | Standard. Docker Compose for dev/staging. K8s for production scale. |

**Monorepo structure:**

```
parallax/
├── crates/                          # Rust workspace
│   ├── px-types/                    # Shared types (zero I/O)
│   ├── px-video/                    # Video decode, normalize, segment
│   ├── px-pose/                     # Pose estimation (ONNX inference)
│   ├── px-track/                    # Multi-object tracking + Re-ID
│   ├── px-calibrate/                # Markerless camera calibration
│   ├── px-temporal/                 # Frame interpolation + super-res
│   ├── px-ball/                     # Ball detection + tracking
│   ├── px-events/                   # Event detection from poses
│   ├── px-encode/                   # Player encoder (inference)
│   ├── px-search/                   # Vector similarity search
│   ├── px-graph/                    # Player/club/league graph
│   ├── px-project/                  # Trajectory prediction (inference)
│   ├── px-risk/                     # Injury + adaptation risk
│   ├── px-fit/                      # Tactical fit scoring
│   ├── px-world/                    # World model (inference)
│   ├── px-api/                      # HTTP API (Axum)
│   ├── px-worker/                   # Job queue consumer
│   └── px-pipeline/                 # End-to-end orchestrator
│
├── training/                        # Python (PyTorch)
│   ├── pose/                        # Pose estimation training
│   ├── reid/                        # Gait Re-ID training
│   ├── encoder/                     # Player encoder training
│   ├── calibrate/                   # Calibration network training
│   ├── temporal/                    # Super-resolution training
│   ├── world_model/                 # World model training
│   └── common/                      # Shared training utilities
│
├── synthetic/                       # Synthetic data engine
│   ├── unity_project/               # Unity scene renderer
│   ├── degradation/                 # Camera degradation simulation
│   ├── motion/                      # Motion generation (MuJoCo)
│   └── curriculum/                  # Training curriculum scheduler
│
├── app/                             # Mobile capture app
│   └── parallax_capture/            # Flutter/RN project
│
├── web/                             # Discovery dashboard
│   └── dashboard/                   # Next.js project
│
├── infra/                           # Deployment
│   ├── docker/
│   ├── k8s/
│   └── terraform/
│
└── docs/
```

---

## 3. Synthetic Data Engine (Priority: Months 1-4)

This is the foundation. Without synthetic data, we can't train robust perception models.

### 3.1 Scene Renderer

**Tech:** Unity HDRP (High Definition Render Pipeline) + ML-Agents

**What it generates:**
- 11v11 football scenes on varied surfaces (dirt, patchy grass, artificial turf, wet grass)
- Varied lighting (equatorial harsh sun, overcast, dusk, indoor, shadows from trees/buildings)
- Varied clothing (matching kits, random streetwear, no shirts, mixed)
- Varied camera positions (sideline, behind goal, elevated, ground-level)
- Varied camera quality (phone-grade distortion, auto-exposure, rolling shutter)
- Background clutter (spectators, fences, trees, buildings, cars)

**Ground truth output per frame:**
- 2D/3D joint positions for every player (33 keypoints each)
- Player identity labels
- Camera extrinsics + intrinsics
- Ball position
- Pitch coordinates for every player
- Event labels (pass, shot, dribble, etc.)

**Scale target:** 100K synthetic matches (each ~10 minutes of simulated play) in first 6 months. This is ~1.7M minutes of training data.

### 3.2 Motion Generator

**Tech:** MuJoCo or Isaac Gym for physics simulation, seeded with motion capture data

**Sources:**
- CMU Motion Capture Database (free, extensive human motion)
- Football-specific MoCap (acquire or record — 20 sessions covers core actions)
- Physics-based procedural generation (randomize parameters within biomechanical constraints)

**Actions to generate:**
- Running (sprint, jog, change of direction, deceleration)
- Dribbling (close control, speed dribble, skill moves)
- Passing (short, long, through ball, cross)
- Shooting (power, finesse, volley, header)
- Defending (tackle, interception, pressing, marking)
- Goalkeeping (save, distribution, positioning)
- Transitions (receiving, turning, shielding)

### 3.3 Degradation Pipeline

Applied on top of rendered scenes to simulate phone-camera artifacts:

| Artifact | Parameters |
|----------|-----------|
| Resolution | 480p to 1080p (phone variation) |
| Frame rate | 15-30fps (phone variation) |
| Compression | H.264/H.265 at variable bitrate (simulates phone encoding) |
| Motion blur | Varies with camera shake amplitude |
| Camera shake | Handheld simulation (Perlin noise on camera transform) |
| Auto-exposure | Simulated AE hunting in changing light |
| Auto-focus | Occasional defocus events |
| Rolling shutter | Per-row time offset |
| Lens distortion | Barrel/pincushion distortion (phone lens models) |
| Noise | ISO noise scaled to lighting conditions |

### 3.4 Curriculum Scheduler

Training progresses through difficulty levels:

```
Level 1: Clean synthetic (perfect conditions)
    ↓
Level 2: Mild degradation (good phone, stable tripod)
    ↓
Level 3: Moderate degradation (average phone, slight shake)
    ↓
Level 4: Heavy degradation (old phone, handheld, poor light)
    ↓
Level 5: Real phone footage (fine-tune on collected data)
    ↓
Level 6: Mixed (all levels randomly sampled — final model)
```

---

## 4. Robust Perception Stack (Priority: Months 2-8)

### 4.1 Pose Estimation

**Architecture:** ViTPose-L (Vision Transformer) fine-tuned for degraded conditions

**Why ViTPose over MediaPipe:**
- MediaPipe: fast but trained on clean data, performance drops 40%+ on degraded input
- ViTPose: transformer architecture, more robust to noise, better at partial occlusion
- ViTPose-L: 307M params, but distillable to ViTPose-S (24M) for mobile/edge

**Training:**
1. Start with COCO/CrowdPose pretrained ViTPose-L checkpoint
2. Fine-tune on synthetic football data (Level 1-2)
3. Progressive curriculum through Levels 3-5
4. Final model handles all conditions

**Output:** 17 COCO keypoints per person per frame (upgrade to 33 HALPE keypoints if finger/foot detail needed)

**Inference:**
- Export to ONNX
- Rust `ort` crate for production inference
- Target: 30fps on M-series Mac, 60fps on GPU server

**Validation metrics:**
- AP (Average Precision) on COCO val set: baseline comparison
- AP on synthetic degraded test set: must be >70% of clean performance
- AP on real phone footage test set: qualitative + quantitative

### 4.2 Player Tracking

**Architecture:** ByteTrack (or BoT-SORT) with custom association

**Challenge:** Standard MOT assumes appearance-based matching (bounding box + ReID features from clothing). On dirt pitches with random clothing, appearance matching fails.

**Solution: Gait-augmented tracking**

```
Frame N poses ──► Hungarian matching on:
                    1. IoU of bounding boxes (short-term)
                    2. Pose similarity (joint angle vector distance)
                    3. Gait embedding similarity (medium-term Re-ID)
                    4. Motion prediction (Kalman filter on pose trajectory)
```

When appearance is unreliable, pose similarity + gait embedding carry the association.

### 4.3 Gait-Based Re-Identification

**This is a key differentiator.** Every person has a unique gait — the way they run, turn, decelerate. In football, this is even more distinctive (dribbling style, body lean, arm movement).

**Architecture:**
- Input: 2-second pose sequence (30-60 frames → normalized skeleton sequence)
- Model: Temporal CNN or Transformer encoder → 128-dim gait embedding
- Training: Contrastive learning (same player across clips = positive pair, different players = negative)
- Data: Synthetic first (ground truth identity), then fine-tune on real broadcast (identity from jersey numbers)

**Key insight:** Gait Re-ID trained on broadcast footage (where identity is known from jersey numbers) transfers to phone footage (where it isn't). The gait signature is the same regardless of video quality.

**Validation:**
- Rank-1 accuracy on synthetic test set: target >90%
- Rank-1 accuracy on real phone footage: target >75%
- Compare against appearance-based ReID (should outperform in degraded conditions)

### 4.4 Markerless Camera Calibration

**The problem:** Standard sports CV assumes visible pitch lines for homography estimation. Dirt pitches have no lines.

**Approach 1: Player-motion calibration**
- Players move in football-shaped distributions over time
- The pitch geometry is implicit in their collective movement patterns
- Train a network that takes 2 minutes of tracked player positions (in pixel space) and predicts camera extrinsics

**Architecture:**
- Input: 2 minutes of 2D tracked positions (all players, in pixel coordinates)
- Model: Set Transformer (permutation-invariant over players) + temporal attention
- Output: Camera rotation (3 DoF), translation (3 DoF), focal length
- Training: Synthetic scenes with known camera params
- Accuracy target: <2m error at pitch center, <5m at edges

**Approach 2: Fallback — manual 4-point calibration**
- User taps 4 known points in the video (corners, center circle, penalty spot)
- Classical homography estimation
- Available in the Showcase app as optional step

**Approach 3: No calibration needed**
- For player-level scouting (not team tactics), we may not need pitch coordinates at all
- Relative metrics (speed relative to other players, spacing relative to nearby players) work in pixel space
- Full calibration only needed for tactical analysis

### 4.5 Temporal Super-Resolution

**The problem:** Phone footage at 15fps misses fast movements (feints, first touch, quick direction changes). These are often the most telling skill indicators.

**Approach:** Biomechanics-constrained frame interpolation

**Architecture:**
- Input: 2 consecutive pose frames
- Model: Physics-informed neural network
- Constraints: max joint angular velocity, limb length preservation, center-of-mass continuity
- Output: N intermediate pose frames (interpolate 15fps → 60fps)

**Why not generic video interpolation (RIFE, etc.)?**
- We don't need pixel-perfect intermediate frames — we need accurate intermediate POSES
- Pose interpolation is a much simpler problem than video interpolation
- Biomechanical constraints make it well-posed (a knee can't rotate 180° in 66ms)

**Training:**
- Start with 60fps synthetic data
- Drop to 15fps, train model to reconstruct missing frames
- Validate against held-out 60fps ground truth

---

## 5. Intelligence Layer (Priority: Months 5-18)

### 5.1 Player Encoder

**This is the atomic asset.** Everything else builds on it.

**Goal:** A 256-dim vector that captures HOW a player plays — not their stats, not their appearance, but their movement signature, decision patterns, and style.

**Architecture:**
```
Input: Per-match pose sequence
    │  (90 min × 25fps = ~135K frames per player)
    │  Subsampled to 5-second windows, 1000 windows per match
    ▼
Window Encoder (per 5-second clip):
    Pose sequence → Spatial-temporal transformer
    Cross-attention with local context (nearby players, ball)
    → 64-dim window embedding
    ▼
Match Aggregator:
    1000 window embeddings → Attention pooling
    → 256-dim match embedding
    ▼
Player Embedding:
    Average or learned aggregation across matches
    → 256-dim player embedding (stable representation)
```

**Training objective:** Self-supervised contrastive learning

- **Positive pairs:** Same player, different matches → embeddings should be close
- **Negative pairs:** Different players → embeddings should be far
- **Hard negatives:** Players in similar positions/roles → embeddings should capture individual style differences

**Critical design choice: Quality-invariant encoding**

The encoder must produce the same embedding whether input comes from broadcast or phone footage.

Training strategy:
1. Train on broadcast pose data (clean, high-quality)
2. Augment with synthetically degraded pose data
3. Contrastive objective: same player from broadcast AND degraded → same embedding
4. Fine-tune on real phone footage once available

**Validation:**
- Retrieval accuracy: given a player's phone-footage embedding, does the broadcast embedding appear in top-5 nearest neighbors?
- Expert validation: do similarity results match domain expert judgment?
- Position clustering: do embeddings naturally cluster by position/role?
- Style discrimination: can embeddings distinguish between stylistically different players in the same position?

### 5.2 Trajectory Predictor

**Goal:** Given a player's embedding trajectory over time, predict where their embedding will be in 6/12/24 months.

**Architecture:**
- Input: Sequence of match embeddings over career (time-stamped)
- Model: State-space model (Mamba) or temporal transformer
- Context: Player age, league difficulty coefficient, minutes played
- Output: Predicted future embedding + uncertainty estimate

**Training:**
- Historical data: players with 3+ years of tracked matches
- Train on first N matches, predict embedding at match N+K
- Validate against held-out future matches

**Key outputs:**
- "This player's trajectory suggests they'll be a top-5-league starter within 18 months"
- "Development has plateaued — embedding hasn't moved in 6 months"
- "Comparable trajectory to [Known Player] at the same age and stage"

### 5.3 Similarity Search

**Tech:** Qdrant (Rust-native vector database) or custom HNSW index

**Indexes:**
- Player embeddings (256-dim, HNSW, cosine similarity)
- Window embeddings (64-dim, for fine-grained skill search)
- Gait embeddings (128-dim, for Re-ID across videos)

**Query types:**
| Query | How it works |
|-------|-------------|
| "Find similar players" | Nearest neighbors in player embedding space |
| "Find players with this specific skill" | Search in window embedding space for specific action types |
| "Players who profile like young [X]" | Find [X]'s historical embedding at age Y, search for current players near that point |
| Natural language | NL → structured query → embedding space filters |

### 5.4 Risk Model

**Goal:** Predict injury risk and adaptation difficulty from movement data.

**Features extracted from pose sequences:**
- Joint angle distributions under load (asymmetry → injury risk)
- Deceleration patterns (abrupt vs. gradual — ACL indicator)
- Workload distribution (overreliance on one leg)
- Recovery movement quality (post-sprint biomechanics)
- Fatigue indicators (movement quality degradation over match duration)

**Model:** Gradient-boosted trees (XGBoost/LightGBM) on biomechanical features

**Adaptation difficulty model:**
- Input: Player embedding + target league/team embedding
- Output: Estimated adaptation time, performance trajectory in new context
- Trained on historical transfer outcomes (labeled by performance before/after transfer)

### 5.5 World Model (Phase 2 — Months 14-24)

**Goal:** A latent dynamics model that predicts how a football match unfolds given an initial state.

**Architecture:**
```
Game State (22 players + ball + velocities)
        │
        ▼
    VQ-VAE Encoder
        │
        ▼
    Latent State (compressed)
        │
        ▼
    Latent Dynamics Transformer
    (predicts next latent state)
        │
        ▼
    Decoder
        │
        ▼
    Predicted Next Game State
```

**Training data:** 100K+ matches with full tracking data (broadcast-derived)

**Applications:**
- Formation simulation: predict outcomes of tactical changes
- Counterfactual analysis: "what if Player X had passed instead of shot?"
- Transfer impact: insert a new player's encoder embedding into a team's game states
- Game prep: simulate opponent tendencies, find exploitable patterns

**This requires full-pitch tracking data (broadcast quality).** Phone footage from a single camera can't provide this. The world model is built on broadcast data and augmented by phone-footage player representations over time.

---

## 6. Data Layer

### 6.1 Storage Architecture

| Store | Tech | Purpose | Scale Target (Year 1) |
|-------|------|---------|----------------------|
| **Video** | S3-compatible (MinIO → AWS S3) | Raw + processed match footage | 100TB |
| **Features** | Apache Parquet on object storage + DuckDB | Per-frame pose data, per-player features | 10TB |
| **Vectors** | Qdrant (self-hosted) | Player/window/gait embeddings | 10M vectors |
| **Graph** | SQLite + custom graph layer | Player ↔ club ↔ league ↔ agent ↔ transfer | 1M nodes |
| **Time-Series** | TimescaleDB | Career trajectories, per-match metrics | 100M data points |
| **Jobs** | Redis + custom Rust workers | Video processing pipeline orchestration | 1K concurrent |
| **Models** | MLflow | Model versions, experiments, A/B | 100 models |
| **Metrics** | Prometheus + Grafana | System + model quality monitoring | — |

### 6.2 Video Processing Pipeline

```
Upload (phone/broadcast)
    │
    ▼
px-video: decode → normalize → segment into clips
    │
    ▼
px-pose: pose estimation (ONNX, ViTPose)
    │
    ▼
px-track: multi-object tracking + gait Re-ID
    │
    ▼
px-calibrate: camera calibration (if needed)
    │
    ▼
px-temporal: frame interpolation (15fps → 60fps)
    │
    ▼
px-ball: ball detection + tracking
    │
    ▼
px-events: event detection (pass, shot, dribble, etc.)
    │
    ▼
px-encode: player encoder (per-player embeddings)
    │
    ▼
Feature Store + Vector DB + Graph
    │
    ▼
Player Profile Generated → Notify user
```

**Processing time target:**
- 90-minute match (phone quality): <30 minutes on GPU server
- 90-minute match (broadcast quality): <15 minutes on GPU server
- Per-player encoding: <5 seconds after tracking

### 6.3 Data Privacy

| Concern | Approach |
|---------|----------|
| Player consent | Required for Showcase profiles. Academy/club uploads require consent from player or guardian. |
| GDPR compliance | Right to deletion. All player data deletable on request. |
| Data residency | EU data stays in EU region (relevant for European leagues) |
| Video retention | Raw video deleted after processing (only features/embeddings retained) unless user opts to keep |
| Anonymization | Embeddings are not directly invertible to video — but gait signatures are biometric. Treat as PII. |

---

## 7. Infrastructure

### 7.1 Compute

| Purpose | Hardware | Provider | Cost Estimate |
|---------|----------|----------|---------------|
| Model training | 4-8x H100 GPU | Lambda / CoreWeave | $15-30K/mo during training |
| Synthetic data rendering | 4x A100 + Unity batch | CoreWeave | $5-10K/mo |
| Production inference | 2-4x A10G or L4 | Hetzner / AWS | $3-8K/mo |
| API + web | Standard compute | Hetzner / Fly.io | $1-2K/mo |
| Storage | Object storage + DB | Hetzner / AWS | $2-5K/mo |

**Total infra cost estimate: $25-55K/mo during training phases, $10-20K/mo in production.**

### 7.2 CI/CD

- GitHub Actions for CI
- Cargo workspace (Rust) — `cargo test --workspace`, `cargo clippy`, `cargo fmt`
- PyTorch training: W&B (Weights & Biases) for experiment tracking
- Docker images built per merge to main
- Staging environment mirrors production
- Model deployment: ONNX export → versioned in model registry → canary rollout

### 7.3 Monitoring

| What | Tool | Alerts |
|------|------|--------|
| System health | Prometheus + Grafana | CPU, memory, GPU util, queue depth |
| Model quality | Custom dashboards | Pose accuracy drift, embedding quality metrics |
| Pipeline throughput | Prometheus | Videos processed/hour, queue wait time |
| API latency | Prometheus | p50/p95/p99 response times |
| User metrics | PostHog or Amplitude | Showcase uploads, Discovery searches, conversion |

---

## 8. Engineering Roadmap

### Phase 1: Foundation (Months 1-3)

**Goal:** Synthetic data engine + basic perception pipeline

| Sprint | Focus | Deliverable |
|--------|-------|-------------|
| 1-2 | Project setup | Rust workspace scaffolded, CI/CD, dev environment |
| 1-3 | Synthetic renderer | Unity project generating football scenes with ground truth |
| 2-3 | Degradation pipeline | Realistic phone-camera artifacts applied to synthetic scenes |
| 2-3 | Motion generator | Basic procedural motion (running, passing) with MoCap seeds |
| 3 | Baseline pose | ViTPose fine-tuned on clean synthetic data |

**Gate:** Synthetic engine producing 1K scenes/day with full ground truth labels.

### Phase 2: Robust Perception (Months 3-6)

**Goal:** Phone-footage pose estimation at near-broadcast quality

| Sprint | Focus | Deliverable |
|--------|-------|-------------|
| 4-5 | Degraded pose | ViTPose fine-tuned through curriculum (clean → degraded) |
| 4-6 | Tracking | ByteTrack + pose-based association working on phone footage |
| 5-6 | Gait Re-ID v1 | Contrastive gait encoder, trained on synthetic + broadcast |
| 5-6 | Markerless calibration v1 | Player-motion calibration network |
| 6 | Integration | End-to-end: phone video in → tracked skeletons + IDs out |

**Gate:** Pose estimation on phone footage within 15% AP of broadcast-quality input. Gait Re-ID rank-1 >75%.

### Phase 3: Player Intelligence (Months 5-9)

**Goal:** Meaningful player embeddings and first profiles

| Sprint | Focus | Deliverable |
|--------|-------|-------------|
| 7-8 | Player encoder v1 | Self-supervised on broadcast pose data |
| 8-9 | Quality-invariant encoding | Phone/broadcast → same embedding (contrastive bridge) |
| 8-9 | Similarity search | Qdrant index, "find similar players" working |
| 9 | Player profiles | Auto-generated cards with movement signature, physical model |
| 9 | Showcase app v1 | Mobile app for video upload + profile viewing |

**Gate:** Domain experts validate similarity results as meaningful. 10+ academy partners testing Showcase.

### Phase 4: Products (Months 9-14)

**Goal:** Paying customers

| Sprint | Focus | Deliverable |
|--------|-------|-------------|
| 10-11 | Discovery dashboard | Search, filter, compare, player detail pages |
| 10-11 | Trajectory predictor v1 | Development curve predictions |
| 11-12 | Risk model v1 | Injury risk + adaptation difficulty |
| 12-13 | Tactical fit v1 | System-fit scoring |
| 13-14 | Sales + onboarding | First paying Discovery customers |

**Gate:** $50K MRR. 200+ academies on Showcase. 50K+ player models.

### Phase 5: World Model + Scale (Months 14-24)

**Goal:** Tactical simulation + Series A

| Sprint | Focus | Deliverable |
|--------|-------|-------------|
| 15-17 | World model v1 | Latent dynamics model on broadcast tracking data |
| 17-19 | Counterfactual engine | "What if?" scenario simulation |
| 18-20 | Tactics product v1 | Formation simulator, game prep reports |
| 20-24 | Scale | Enterprise API, custom models, transfer advisory |

**Gate:** $200K+ MRR. World model generating plausible 10-second rollouts. Series A raised.

---

## 9. Model Inventory

| Model | Architecture | Params | Input | Output | ONNX Export |
|-------|-------------|--------|-------|--------|-------------|
| Pose estimator | ViTPose-L (→ ViTPose-S for mobile) | 307M (→ 24M) | RGB frame | 17 keypoints per person | Yes |
| Tracker | ByteTrack + Kalman | <1M | Detections + poses | Track IDs | Custom Rust |
| Gait Re-ID | Temporal CNN/Transformer | ~10M | 2s pose sequence | 128-dim embedding | Yes |
| Calibration net | Set Transformer | ~5M | 2min tracked positions | Camera extrinsics | Yes |
| Temporal super-res | Physics-informed MLP | ~2M | 2 consecutive poses | N intermediate poses | Yes |
| Ball detector | TrackNetV3 | ~5M | 3 consecutive frames | Ball position | Yes |
| Event detector | ActionFormer variant | ~15M | Pose sequence + ball | Event labels | Yes |
| Player encoder | Spatial-temporal transformer | ~50M | Match pose sequence | 256-dim embedding | Yes |
| Trajectory predictor | Mamba / SSM | ~20M | Embedding sequence | Future embedding | Yes |
| Risk model | XGBoost | <1M | Biomechanical features | Risk scores | ONNX (treelite) |
| World model | VQ-VAE + dynamics transformer | ~200M | Game state sequence | Next game states | Yes |

**Total model footprint (inference):** ~300M params for full stack (excluding world model). ONNX quantized (INT8): ~300MB total. Fits comfortably on a single A10G GPU.

---

## 10. Research Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Pose estimation too noisy on phone footage | Medium | Critical | Synthetic data curriculum. Progressive quality tiers — even 70% accuracy is useful for scouting. Ensemble with multiple pose models. |
| Gait Re-ID doesn't work reliably | Medium | High | Fall back to manual tagging in Showcase app. Gait improves with more data. Use appearance-based Re-ID where possible (kits available). |
| Player encoder produces meaningless embeddings | Low-Medium | Critical | Start with well-understood contrastive learning. Validate with domain experts every 2 weeks. Fall back to explicit feature engineering (handcrafted metrics) if learned representations fail. |
| Sim-to-real gap too large | Medium | High | Domain randomization + real-data fine-tuning. Collect real phone footage from pilot academies as early as possible. |
| World model doesn't produce useful simulations | Medium | Medium | World model is Phase 2. Scout product stands alone without it. Start with simpler baselines (probabilistic models) before full neural world model. |

---

## 11. Key Technical Decisions to Make Early

| Decision | Options | Recommendation | Decide By |
|----------|---------|---------------|-----------|
| Render engine | Unity HDRP vs. Unreal Engine 5 | Unity — ML-Agents integration, faster iteration, adequate visual quality | Month 1 |
| Pose model | ViTPose vs. HRNet vs. MediaPipe | ViTPose-L — best robustness/accuracy tradeoff, transformer architecture | Month 2 |
| Tracker | ByteTrack vs. BoT-SORT vs. custom | ByteTrack + custom gait association — proven, extensible | Month 3 |
| Player encoder architecture | Transformer vs. SSM vs. CNN | Transformer with cross-attention — most flexible, proven on sequence tasks | Month 5 |
| Vector DB | Qdrant vs. Milvus vs. custom | Qdrant — Rust-native, self-hostable, good API | Month 6 |
| Mobile framework | Flutter vs. React Native | Flutter — better camera APIs, single codebase | Month 7 |
| World model architecture | VQ-VAE + transformer vs. diffusion vs. GAN | VQ-VAE + transformer — most stable training, interpretable latent space | Month 14 |
