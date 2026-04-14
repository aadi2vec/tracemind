//! TraceMind Embedding & Retrieval Benchmark
//!
//! A quality evaluation harness for embeddings and retrieval that runs as a
//! nightly CI test. NOT packaged in the DMG.
//!
//! Sections:
//! 1. Embedding quality — cosine similarity margin on (query, pos, neg) triples
//! 2. Retrieval quality — P@3, P@5, MRR, latency on a synthetic corpus
//! 3. Model comparison (optional, `--compare` flag) — side-by-side table
//!
//! Exit code: 0 = PASS, 1 = FAIL (for CI integration).

use std::path::PathBuf;
use std::time::Instant;

use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;
use tm_types::Entity;
use tm_vector::Embedder;
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════
// Pass/fail thresholds
// ═══════════════════════════════════════════════════════════════════════════

const THRESHOLD_MARGIN: f64 = 0.3;
const THRESHOLD_ACCURACY: f64 = 90.0;
const THRESHOLD_PRECISION_AT_3: f64 = 0.2;

// ═══════════════════════════════════════════════════════════════════════════
// Embedding quality triples: (query, positive_doc, negative_doc)
// ═══════════════════════════════════════════════════════════════════════════

const EMBEDDING_TRIPLES: &[(&str, &str, &str)] = &[
    (
        "What is Rust?",
        "Rust is a systems programming language focused on safety and performance",
        "The weather today is sunny and warm",
    ),
    (
        "machine learning frameworks",
        "PyTorch and TensorFlow are popular deep learning frameworks",
        "Cooking recipes for Italian pasta dishes",
    ),
    (
        "how does TCP work",
        "TCP uses a three-way handshake to establish reliable connections",
        "Shakespeare wrote many famous plays",
    ),
    (
        "database indexing strategies",
        "B-tree indexes speed up range queries in SQL databases",
        "The history of ancient Rome",
    ),
    (
        "functional programming",
        "Haskell uses monads for handling side effects in pure functions",
        "How to train for a marathon",
    ),
    (
        "kubernetes deployment",
        "Pods are the smallest deployable units in Kubernetes clusters",
        "Classical music compositions by Beethoven",
    ),
    (
        "graph neural networks",
        "GNNs aggregate neighbor features through message passing layers",
        "Best hiking trails in Colorado",
    ),
    (
        "memory management in C",
        "malloc allocates heap memory that must be freed to avoid leaks",
        "Popular tourist destinations in Japan",
    ),
    (
        "distributed consensus",
        "Raft elects a leader to coordinate log replication across nodes",
        "How to grow tomatoes in a garden",
    ),
    (
        "transformer architecture",
        "Self-attention computes weighted sums over all input positions",
        "The rules of chess",
    ),
    (
        "REST API design",
        "RESTful endpoints use HTTP verbs like GET POST PUT DELETE",
        "Recipes for homemade bread",
    ),
    (
        "type systems",
        "Hindley-Milner type inference enables polymorphism without annotations",
        "Bird watching in North America",
    ),
    (
        "concurrency patterns",
        "Channels in Go provide CSP-style communication between goroutines",
        "Interior decorating tips",
    ),
    (
        "vector databases",
        "HNSW index enables approximate nearest neighbor search in high dimensions",
        "How to play guitar",
    ),
    (
        "cryptographic hashing",
        "SHA-256 produces a fixed 256-bit digest from arbitrary input data",
        "Gardening tools and supplies",
    ),
];

// ═══════════════════════════════════════════════════════════════════════════
// Retrieval corpus: documents to ingest
// ═══════════════════════════════════════════════════════════════════════════

const RETRIEVAL_DOCS: &[&str] = &[
    // Programming languages
    "Rust is a systems programming language that guarantees memory safety without garbage collection. It uses ownership and borrowing to prevent data races at compile time.",
    "Python is a high-level interpreted language popular for scripting, data science, and machine learning. It has a large ecosystem of libraries like NumPy and pandas.",
    "Go is a statically typed compiled language designed at Google for building scalable network services. It features goroutines for lightweight concurrency.",
    "JavaScript is the language of the web, running in browsers and on servers via Node.js. It supports event-driven, non-blocking I/O for high-throughput applications.",
    "C is a low-level systems programming language that provides direct memory access through pointers. It requires manual memory management with malloc and free.",
    "TypeScript adds static type checking to JavaScript, catching errors at compile time. It compiles down to plain JavaScript and works with existing JS ecosystems.",
    // ML frameworks
    "PyTorch is a deep learning framework that uses dynamic computation graphs, making it easy to debug and modify models on the fly. It is widely used in research.",
    "TensorFlow is Google's machine learning framework that supports both eager and graph execution modes. TensorFlow Serving enables production model deployment.",
    "scikit-learn provides classical machine learning algorithms like random forests, SVMs, and k-means clustering. It is built on top of NumPy and SciPy.",
    "JAX is a numerical computing library from Google that provides automatic differentiation and XLA compilation for high-performance machine learning research.",
    // Databases
    "PostgreSQL is an advanced open-source relational database with support for JSONB, full-text search, and extensions like PostGIS for geospatial queries.",
    "Redis is an in-memory data structure store used as a cache, message broker, and database. It supports strings, hashes, lists, sets, and sorted sets.",
    "MongoDB is a document-oriented NoSQL database that stores data as flexible JSON-like BSON documents. It supports horizontal scaling through sharding.",
    "SQLite is an embedded relational database that stores the entire database in a single file. It requires no server process and is ideal for local applications.",
    // Infrastructure
    "Docker containers package applications with their dependencies into isolated, reproducible units. Dockerfiles define the build steps for container images.",
    "Kubernetes orchestrates container deployments across clusters of machines. It manages scaling, rolling updates, and service discovery through declarative configs.",
    "AWS Lambda provides serverless compute that runs code in response to events without provisioning servers. It scales automatically and charges per invocation.",
    "Terraform is an infrastructure-as-code tool that provisions cloud resources declaratively. It supports AWS, GCP, Azure, and many other providers.",
    // Additional context docs
    "Nginx is a high-performance web server and reverse proxy. It handles load balancing, SSL termination, and static file serving efficiently.",
    "GraphQL is a query language for APIs that lets clients request exactly the data they need. It provides a single endpoint instead of multiple REST routes.",
    "gRPC is a high-performance RPC framework that uses Protocol Buffers for serialization. It supports streaming and works across many programming languages.",
    "Prometheus collects time-series metrics from instrumented applications. It uses PromQL for querying and integrates with Grafana for visualization.",
    "Git is a distributed version control system that tracks changes to source code. It supports branching, merging, and collaboration through remote repositories.",
    "Linux is an open-source operating system kernel used in servers, embedded systems, and desktops. It provides process management, memory management, and file systems.",
];

// ═══════════════════════════════════════════════════════════════════════════
// Retrieval queries with expected relevant entity names (lowercased substrings)
// ═══════════════════════════════════════════════════════════════════════════

struct RetrievalQuery {
    query: &'static str,
    /// Lowercased substrings that must appear in a relevant entity name.
    expected: &'static [&'static str],
}

const RETRIEVAL_QUERIES: &[RetrievalQuery] = &[
    RetrievalQuery {
        query: "What programming language is best for systems programming?",
        expected: &["rust", "c"],
    },
    RetrievalQuery {
        query: "Which ML framework uses dynamic computation graphs?",
        expected: &["pytorch"],
    },
    RetrievalQuery {
        query: "How to cache data in memory?",
        expected: &["redis"],
    },
    RetrievalQuery {
        query: "What is the best database for geospatial queries?",
        expected: &["postgresql", "postgis", "postgres"],
    },
    RetrievalQuery {
        query: "How to deploy containers at scale?",
        expected: &["kubernetes", "docker"],
    },
    RetrievalQuery {
        query: "What tool provisions cloud infrastructure declaratively?",
        expected: &["terraform"],
    },
    RetrievalQuery {
        query: "Which language is used for web frontend development?",
        expected: &["javascript", "typescript"],
    },
    RetrievalQuery {
        query: "How to do machine learning with classical algorithms?",
        expected: &["scikit", "sklearn", "scikit-learn"],
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// Cosine similarity
// ═══════════════════════════════════════════════════════════════════════════

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "vector dimension mismatch");
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| *x as f64 * *y as f64).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 1: Embedding Quality
// ═══════════════════════════════════════════════════════════════════════════

struct EmbeddingResults {
    count: usize,
    avg_sim_pos: f64,
    avg_sim_neg: f64,
    margin: f64,
    accuracy: f64,
    correct: usize,
}

fn run_embedding_quality(embedder: &Embedder) -> EmbeddingResults {
    let mut sum_sim_pos = 0.0;
    let mut sum_sim_neg = 0.0;
    let mut correct = 0usize;

    for &(query, pos, neg) in EMBEDDING_TRIPLES {
        let q_vec = embedder.embed(query);
        let p_vec = embedder.embed(pos);
        let n_vec = embedder.embed(neg);

        let sim_pos = cosine_similarity(&q_vec, &p_vec);
        let sim_neg = cosine_similarity(&q_vec, &n_vec);

        sum_sim_pos += sim_pos;
        sum_sim_neg += sim_neg;

        if sim_pos > sim_neg {
            correct += 1;
        }
    }

    let count = EMBEDDING_TRIPLES.len();
    let avg_sim_pos = sum_sim_pos / count as f64;
    let avg_sim_neg = sum_sim_neg / count as f64;

    EmbeddingResults {
        count,
        avg_sim_pos,
        avg_sim_neg,
        margin: avg_sim_pos - avg_sim_neg,
        accuracy: (correct as f64 / count as f64) * 100.0,
        correct,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 2: Retrieval Quality
// ═══════════════════════════════════════════════════════════════════════════

struct RetrievalResults {
    query_count: usize,
    precision_at_3: f64,
    precision_at_5: f64,
    mrr: f64,
    avg_latency_ms: f64,
}

/// Check if an entity name is relevant to the query's expected set.
fn is_relevant(entity: &Entity, expected: &[&str]) -> bool {
    let name_lower = entity.name.to_lowercase();
    expected.iter().any(|e| name_lower.contains(e))
}

fn run_retrieval_quality(tmp_dir: &PathBuf, use_hash: bool) -> RetrievalResults {
    let db_path = tmp_dir.join("bench.db");
    let trace_path = tmp_dir.join("traces.jsonl");
    let db_str = db_path.to_str().unwrap();
    let trace_str = trace_path.to_str().unwrap();

    // Ingest all documents
    let pipeline = IngestPipeline::open(db_str, use_hash)
        .expect("failed to open IngestPipeline");
    let session = Uuid::new_v4();

    for doc in RETRIEVAL_DOCS {
        if let Err(e) = pipeline.ingest(doc, session) {
            eprintln!("  warn: ingest failed for doc: {e}");
        }
    }

    // Open retrieval engine
    let mut engine = RetrievalEngine::open(db_str, trace_str, use_hash)
        .expect("failed to open RetrievalEngine");

    let mut total_p3 = 0.0;
    let mut total_p5 = 0.0;
    let mut total_rr = 0.0;
    let mut total_latency = std::time::Duration::ZERO;
    let mut query_count = 0usize;

    for rq in RETRIEVAL_QUERIES {
        let start = Instant::now();
        let result = match engine.query(rq.query) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  warn: query {:?} failed: {e}", rq.query);
                continue;
            }
        };
        let elapsed = start.elapsed();
        total_latency += elapsed;
        query_count += 1;

        let entities = &result.entities;

        // Precision@3
        let top3: Vec<&Entity> = entities.iter().take(3).collect();
        let rel3 = top3.iter().filter(|e| is_relevant(e, rq.expected)).count();
        total_p3 += rel3 as f64 / 3.0_f64.min(top3.len() as f64).max(1.0);

        // Precision@5
        let top5: Vec<&Entity> = entities.iter().take(5).collect();
        let rel5 = top5.iter().filter(|e| is_relevant(e, rq.expected)).count();
        total_p5 += rel5 as f64 / 5.0_f64.min(top5.len() as f64).max(1.0);

        // MRR: 1/rank of first relevant result
        let first_rel_rank = entities
            .iter()
            .enumerate()
            .find(|(_, e)| is_relevant(e, rq.expected))
            .map(|(i, _)| i + 1);
        if let Some(rank) = first_rel_rank {
            total_rr += 1.0 / rank as f64;
        }
        // If no relevant result found, reciprocal rank = 0 (already not added)
    }

    if query_count == 0 {
        return RetrievalResults {
            query_count: 0,
            precision_at_3: 0.0,
            precision_at_5: 0.0,
            mrr: 0.0,
            avg_latency_ms: 0.0,
        };
    }

    let n = query_count as f64;
    RetrievalResults {
        query_count,
        precision_at_3: total_p3 / n,
        precision_at_5: total_p5 / n,
        mrr: total_rr / n,
        avg_latency_ms: total_latency.as_secs_f64() * 1000.0 / n,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 3: Model comparison (--compare)
// ═══════════════════════════════════════════════════════════════════════════

fn run_comparison() {
    use tm_vector::EmbedModel;

    println!();
    println!("{}", "=".repeat(70));
    println!("  Model Comparison (all Apache 2.0, 384-dim)");
    println!("{}", "=".repeat(70));
    println!();

    println!(
        "  {:<30} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "Model", "Sim+", "Sim-", "Margin", "Acc%", "ms/emb"
    );
    println!("  {}", "-".repeat(72));

    // Hash-based baseline
    let hash_embedder = Embedder::new_hash();
    let hash_start = Instant::now();
    let hash_results = run_embedding_quality(&hash_embedder);
    let hash_ms = hash_start.elapsed().as_millis() as f64 / (hash_results.count as f64 * 3.0);

    println!(
        "  {:<30} {:>8.3} {:>8.3} {:>8.3} {:>7.1}% {:>7.2}",
        "hash-baseline", hash_results.avg_sim_pos, hash_results.avg_sim_neg,
        hash_results.margin, hash_results.accuracy, hash_ms,
    );

    // All real models
    for model in EmbedModel::all() {
        match Embedder::with_model(*model) {
            Ok(embedder) => {
                let start = Instant::now();
                let results = run_embedding_quality(&embedder);
                let ms_per = start.elapsed().as_millis() as f64 / (results.count as f64 * 3.0);

                println!(
                    "  {:<30} {:>8.3} {:>8.3} {:>8.3} {:>7.1}% {:>7.2}",
                    model.to_string(), results.avg_sim_pos, results.avg_sim_neg,
                    results.margin, results.accuracy, ms_per,
                );
            }
            Err(e) => {
                eprintln!("  {:<30} SKIP ({})", model.to_string(), e);
            }
        }
    }

    println!();
}

// ═══════════════════════════════════════════════════════════════════════════
// Output formatting
// ═══════════════════════════════════════════════════════════════════════════

fn print_report(
    model_name: &str,
    embed: &EmbeddingResults,
    retrieval: &RetrievalResults,
) -> bool {
    let date = chrono_date_now();

    let bar = "=".repeat(55);
    println!();
    println!("{bar}");
    println!("  TraceMind Embedding & Retrieval Benchmark");
    println!("  Model: {model_name}");
    println!("  Date: {date}");
    println!("{bar}");

    println!();
    println!("{} Embedding Quality {}", "\u{2500}\u{2500}", "\u{2500}".repeat(36));
    println!("  Triples evaluated:  {}", embed.count);
    println!("  Avg sim(q, pos):    {:.3}", embed.avg_sim_pos);
    println!("  Avg sim(q, neg):    {:.3}", embed.avg_sim_neg);
    println!("  Margin:             {:.3}", embed.margin);
    println!(
        "  Accuracy:           {:.1}% ({}/{})",
        embed.accuracy, embed.correct, embed.count
    );

    println!();
    println!("{} Retrieval Quality {}", "\u{2500}\u{2500}", "\u{2500}".repeat(36));
    println!("  Queries evaluated:  {}", retrieval.query_count);
    println!("  Precision@3:        {:.3}", retrieval.precision_at_3);
    println!("  Precision@5:        {:.3}", retrieval.precision_at_5);
    println!("  MRR:                {:.3}", retrieval.mrr);
    println!("  Avg latency:        {:.1}ms", retrieval.avg_latency_ms);

    // Determine pass/fail
    let margin_ok = embed.margin >= THRESHOLD_MARGIN;
    let acc_ok = embed.accuracy >= THRESHOLD_ACCURACY;
    let p3_ok = retrieval.precision_at_3 >= THRESHOLD_PRECISION_AT_3;
    let pass = margin_ok && acc_ok && p3_ok;

    println!();
    println!("{} Summary {}", "\u{2500}\u{2500}", "\u{2500}".repeat(46));
    if pass {
        println!(
            "  PASS: margin > {THRESHOLD_MARGIN}, accuracy > {THRESHOLD_ACCURACY}%, P@3 > {THRESHOLD_PRECISION_AT_3}"
        );
    } else {
        let mut failures = Vec::new();
        if !margin_ok {
            failures.push(format!(
                "margin {:.3} < {THRESHOLD_MARGIN}",
                embed.margin
            ));
        }
        if !acc_ok {
            failures.push(format!(
                "accuracy {:.1}% < {THRESHOLD_ACCURACY}%",
                embed.accuracy
            ));
        }
        if !p3_ok {
            failures.push(format!(
                "P@3 {:.3} < {THRESHOLD_PRECISION_AT_3}",
                retrieval.precision_at_3
            ));
        }
        println!("  FAIL: {}", failures.join(", "));
    }
    println!("{bar}");
    println!();

    pass
}

/// Simple date string without pulling in chrono as a dependency.
fn chrono_date_now() -> String {
    // Use std::process::Command to get date, fallback to "unknown"
    std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// Main
// ═══════════════════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let compare = args.iter().any(|a| a == "--compare");
    let use_hash = args.iter().any(|a| a == "--hash");

    // Determine which embedder to benchmark.
    // --hash for hash baseline, --model=<name> for a specific model, default: BGE
    let model_arg = args.iter()
        .find(|a| a.starts_with("--model="))
        .and_then(|a| tm_vector::EmbedModel::from_str_loose(&a["--model=".len()..]));

    let (embedder, model_name) = if use_hash {
        (Embedder::new_hash(), "hash-baseline (384-dim)".to_string())
    } else {
        let model = model_arg.unwrap_or_default();
        match Embedder::with_model(model) {
            Ok(e) => {
                let name = format!("{} (384-dim)", model);
                (e, name)
            }
            Err(err) => {
                eprintln!("warn: model {model} unavailable ({err}), falling back to hash");
                (Embedder::new_hash(), "hash-baseline (384-dim, fallback)".to_string())
            }
        }
    };

    // Section 1: Embedding quality
    let embed_results = run_embedding_quality(&embedder);

    // Section 2: Retrieval quality (uses a temp directory)
    let tmp_dir = std::env::temp_dir().join(format!("tm-bench-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).expect("failed to create temp dir");
    let retrieval_results = run_retrieval_quality(&tmp_dir, use_hash);

    // Clean up temp dir
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // Print report
    let pass = print_report(&model_name, &embed_results, &retrieval_results);

    // Section 3: Model comparison (optional)
    if compare {
        run_comparison();
    }

    // Exit code: 0 = PASS, 1 = FAIL
    std::process::exit(if pass { 0 } else { 1 });
}
