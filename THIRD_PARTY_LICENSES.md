# Third-party model licenses

TraceMind ships with the following pretrained model weights bundled into
its desktop distribution (macOS `.dmg`, etc.). Each is redistributed under
its own license; source attributions below.

Binary-only redistribution in a packaged app is permitted by every license
listed here, but downstream forks are responsible for reviewing each
license text themselves. Source URLs are the canonical references.

---

## GLiNER small v2.1 (ONNX, int8 quantized)

- **Bundle location:** `models/models--onnx-community--gliner_small-v2.1/`
- **Used by:** `GlinerExtractor` in `tm-ingest` (named-entity recognition).
- **Upstream (ONNX export):** <https://huggingface.co/onnx-community/gliner_small-v2.1>
- **Upstream (original):** <https://huggingface.co/urchade/gliner_small-v2.1>
- **License:** Apache 2.0
- **Citation:**
  > Zaratiana, U., Tomeh, N., Holat, P., & Charnois, T. (2023).
  > *GLiNER: Generalist Model for Named Entity Recognition using
  > Bidirectional Transformer.* arXiv:2311.08526.

## mxbai-edge-colbert-v0 (17M, ONNX)

- **Bundle location:** `models/models--mixedbread-ai--mxbai-edge-colbert-v0-17m/`
- **Used by:** `ColbertReranker` in `tm-rerank` (late-interaction reranking).
- **Upstream:** <https://huggingface.co/mixedbread-ai/mxbai-edge-colbert-v0-17m>
- **License:** Apache 2.0
- **Attribution:** Mixedbread AI, 2024.

## BGE-small-en-v1.5 (ONNX)

- **Bundle location:** `models/models--Xenova--bge-small-en-v1.5/`
- **Used by:** `Embedder` in `tm-vector` via `fastembed` (text embeddings,
  384-dim).
- **Upstream (ONNX export):** <https://huggingface.co/Xenova/bge-small-en-v1.5>
- **Upstream (original):** <https://huggingface.co/BAAI/bge-small-en-v1.5>
- **License:** MIT
- **Citation:**
  > Xiao, S., Liu, Z., Zhang, P., & Muennighoff, N. (2023).
  > *C-Pack: Packaged Resources To Advance General Chinese Embedding.*
  > arXiv:2309.07597.

---

## Rust crate licenses

All Rust dependencies are listed in `Cargo.lock`. Notable direct dependencies
used by the model bundling path:

- `ort` (Apache 2.0 OR MIT) — ONNX Runtime bindings.
- `tokenizers` (Apache 2.0) — HuggingFace tokenizer Rust port.
- `fastembed` (Apache 2.0) — ONNX embedding loader + cache manager.
- `hf-hub` (Apache 2.0) — HuggingFace Hub client for Rust.

Full dependency licenses can be regenerated with `cargo about generate` or
inspected via `cargo tree`.
