# Product Close — One Engine, Three Products

**Audience:** the last 30 seconds of the recordable demo (D-6 of the punch
list) and the matching investor-deck slide. Designed to be shot as a
single static screen.

---

## The one-pager

```
┌───────────────────────────────────────────────────────────────────────────┐
│  ONE ENGINE.  THREE PRODUCTS.                                             │
│                                                                           │
│  TRACEMIND   personal memory OS            →  the product on screen now   │
│  ENGRAM      memory SDK for agents         →  what apps and agents embed  │
│  ROSETTA     semantic code memory          →  what teams use over a repo  │
│                                                                           │
│  ────────────────────────────────────────────────────────────────────     │
│                                                                           │
│  All three share:                                                         │
│      tm-graph      bitemporal knowledge graph                             │
│      tm-tms        justification-based truth maintenance                  │
│      tm-temporal   point-in-time queries                                  │
│      tm-intent     commitments + outcomes + patterns                      │
│      tm-reflect    daily brief + insights                                 │
│      tm-world-model    on-device outcome prediction                       │
│                                                                           │
│  Local-only by default. Fine-tuneable on-device.                          │
│  Ship it on your laptop, your phone, your team's repo.                    │
│                                                                           │
│  ────────────────────────────────────────────────────────────────────     │
│                                                                           │
│      tracemind.dev    ·    curl tracemind.dev/install.sh | sh             │
└───────────────────────────────────────────────────────────────────────────┘
```

---

## Product breakdown

### TraceMind — the personal memory OS *(shipping)*

What the demo just walked through.

- **Wedge:** the system of intents. The brief that knows what's overdue,
  resolves contradictions in your own knowledge, asks for outcomes.
- **Surface:** Tauri desktop app, CLI, MCP for Claude Code.
- **User:** an individual — founder, researcher, journalist, knowledge
  worker.
- **Distribution:** `curl … | sh`. Single binary. No login, no cloud.

### Engram — the memory SDK *(next)*

The same engine, exposed as an embeddable Rust crate + a thin Python /
TS wrapper.

- **Wedge:** agents that *remember* across sessions, not just retrieve
  over a vector store. Contradiction-aware. Bitemporal.
- **Surface:** `cargo add engram` / `pip install engram`.
- **User:** an AI engineer building an agent or a copilot.
- **Distribution:** package registry. Local-first stays the default; a
  hosted control plane is opt-in.

### Rosetta — semantic code memory *(later)*

Same engine, indexed over a code repository instead of a personal
knowledge graph.

- **Wedge:** "why was this written this way?" answered from the code's
  *commit, review, and incident history* — not a vector search over the
  current source tree.
- **Surface:** CLI + IDE extension + GitHub app.
- **User:** an engineering team.
- **Distribution:** self-hosted, repo-local. Optional team server.

---

## Why this is the right close

Three observations:

1. **The hard part is the engine, not the surface.** Bitemporal graph,
   JTMS, intents, world-model — those are the load-bearing pieces.
   They're shared across all three products.
2. **Each product is a different *user*, not a different *technology*.**
   Personal vs agent-builder vs eng-team. The engine doesn't change.
3. **Local-only defaults compound.** Every product gets the same
   privacy posture for free. We never have to reverse a "we own your
   data" gravity later.

---

## Hard constraints for the on-screen render

- Three rows. Three names. Three taglines. No more.
- The shared-engine list stays short — six crate names, each one a
  concept, no acronyms exploded.
- The end card carries the install one-liner and the domain.
- No screenshots of the deck inside the demo; the demo's last frame is
  this one.
