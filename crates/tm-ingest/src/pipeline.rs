use seahash;
use uuid::Uuid;

use tm_types::{Entity, EntityType, Predicate, Result, Trace, TraceEventType, Triple};
use tm_graph::GraphStore;
use tm_vector::{Embedder, VectorStore};
use tm_governance::GovernanceFilter;

pub struct IngestPipeline {
    graph: GraphStore,
    vector: VectorStore,
    embedder: Embedder,
    governance: GovernanceFilter,
}

#[derive(Debug)]
pub struct IngestResult {
    pub trace: Trace,
    pub entities: Vec<Entity>,
    pub triples: Vec<Triple>,
    pub content_hash: String,
}

impl IngestPipeline {
    /// Open graph store at `db_path`, vector store at `db_path` + ".vec",
    /// and initialise embedder and default governance filter.
    ///
    /// If `hash_embed` is true, uses the deterministic hash embedder (no model download).
    pub fn open(db_path: &str, hash_embed: bool) -> Result<Self> {
        let graph = GraphStore::open(db_path)?;
        let vec_path = format!("{}.vec", db_path);
        let vector = VectorStore::open(&vec_path)?;
        let embedder = if hash_embed {
            Embedder::new_hash()
        } else {
            Embedder::new()?
        };
        let governance = GovernanceFilter::default();

        Ok(Self {
            graph,
            vector,
            embedder,
            governance,
        })
    }

    /// Ingest raw `text` from `session_id`:
    ///
    /// 1. Run governance check (confidence = 1.0).
    /// 2. Hash the text.
    /// 3. Extract entities heuristically (multi-word aware).
    /// 4. Upsert each entity to graph + vector stores (context-aware embeddings).
    /// 5. Extract typed triples via pattern matching + co-occurrence fallback.
    /// 6. Upsert each triple to the graph store.
    /// 7. Return an `IngestResult`.
    pub fn ingest(&self, text: &str, session_id: Uuid) -> Result<IngestResult> {
        // 1. Governance check.
        self.governance.check(text, 1.0)?;

        // 2. Hash.
        let content_hash = hash_text(text);

        // 3. Extract entities (multi-word aware).
        let entities = extract_entities(text);

        // 4. Upsert entities with context-aware embeddings.
        //    Embed "entity_name: full source text" so the vector captures
        //    the semantic context in which the entity appeared.
        for entity in &entities {
            self.graph.upsert_entity(entity)?;
            let embed_text = format!("{}: {}", entity.name, text);
            let embedding = self.embedder.embed(&embed_text);
            self.vector.upsert(entity.id, &embedding)?;
        }

        // 5. Extract typed triples via pattern matching, then fill with co-occurrence.
        let triples = extract_triples(text, &entities);

        // 6. Upsert triples to graph.
        for triple in &triples {
            self.graph.upsert_triple(triple)?;
        }

        // 7. Build trace record with full provenance.
        let mut trace = Trace::new(session_id, TraceEventType::Ingest, &content_hash);
        trace.raw_text = Some(text.to_string());
        trace.entities_extracted = entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = triples.iter().map(|t| t.id).collect();

        Ok(IngestResult {
            trace,
            entities,
            triples,
            content_hash,
        })
    }
}

// ---------------------------------------------------------------------------
// Heuristic NER — multi-word aware
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "The", "A", "An", "In", "On", "At", "To", "For", "Of", "And", "Or", "But", "Is", "Was",
    "Are", "Were", "Be", "Been", "It", "Its", "This", "That", "With", "From", "By", "As", "Up",
    "Out", "If", "So", "No", "I", "My", "We", "Our", "He", "She", "They", "You", "Your",
    "Has", "Had", "Have", "Do", "Does", "Did", "Not", "All", "Each", "Every", "Can", "Will",
    "Just", "Now", "Then", "Here", "There", "When", "How", "What", "Who", "Which", "Where",
];

/// Known technology/language names that should never be classified as Person.
const KNOWN_TECH: &[&str] = &[
    "Rust", "Python", "JavaScript", "TypeScript", "React", "Docker", "Kubernetes", "Linux",
    "Redis", "Postgres", "PostgreSQL", "MongoDB", "SQLite", "Tauri", "Wasm", "WebAssembly",
    "Git", "GitHub", "Node", "Deno", "Cargo", "Webpack", "Vite", "FastAPI", "Django", "Flask",
    "Spring", "Java", "Kotlin", "Swift", "Go", "Ruby", "Rails", "Vue", "Angular", "Svelte",
    "AWS", "Azure", "GCP", "Terraform", "Ansible", "Nginx", "Apache", "GraphQL", "REST",
    "LanceDB", "ChromaDB", "Pinecone", "Qdrant", "ONNX", "PyTorch", "TensorFlow",
    "Claude", "GPT", "LLM", "MCP", "API", "CLI", "SDK", "CSS", "HTML", "SQL",
    "Privacy", "Security", "Performance", "Latency", "Throughput", "Memory", "CPU", "GPU",
];

/// Organisation suffixes — if a multi-word entity ends with one of these, classify as Org.
const ORG_SUFFIXES: &[&str] = &[
    "Inc", "Corp", "LLC", "Ltd", "Co", "Company", "Foundation", "Institute", "Labs",
    "Technologies", "Systems", "Group", "Team", "Studio", "Studios",
];

const FILE_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".js", ".ts", ".go", ".md", ".txt", ".json", ".toml",
    ".yaml", ".yml", ".html", ".css", ".sql", ".sh", ".dockerfile",
];

const STRIP_CHARS: &[char] = &[',', '.', '!', '?', ';', ':', '"', '\'', '(', ')'];

/// Common English verbs, adjectives, and functional words that should never become entities.
const SKIP_WORDS: &[&str] = &[
    // Verbs
    "uses", "using", "used", "works", "working", "worked", "built", "builds", "building",
    "stores", "storing", "stored", "runs", "running", "creates", "creating", "created",
    "makes", "making", "calls", "calling", "called", "enables", "enabling", "enabled",
    "exposes", "exposing", "exposed", "backs", "backing", "backed", "leaves", "leaving",
    "gets", "getting", "sets", "setting", "adds", "adding", "sends", "sending",
    "takes", "taking", "gives", "giving", "goes", "going", "comes", "coming",
    "keeps", "keeping", "finds", "finding", "tells", "telling", "says", "saying",
    "shows", "showing", "means", "meaning", "tries", "trying", "starts", "starting",
    "turns", "turning", "plays", "playing", "moves", "moving", "lives", "living",
    "believes", "happens", "writes", "provides", "includes", "continues", "allows",
    "produces", "needs", "helps", "reads", "holds", "generates", "brings", "mentions",
    "develops", "developing", "developed", "implements", "implementing", "implemented",
    "collaborates", "collaborating",
    // Past participles / adjectives
    "based", "designed", "focused", "related", "known", "open", "local", "only",
    "also", "even", "just", "very", "most", "more", "much", "many", "some", "such",
    "well", "still", "already", "always", "never", "ever", "often", "really",
    // Functional
    "across", "between", "through", "within", "without", "about", "after", "before",
    "over", "under", "into", "like", "than", "both", "each", "other", "while",
    "during", "since", "until", "against", "among", "along", "around",
    // Common nouns too generic to be useful
    "data", "time", "information", "system", "systems", "tool", "tools", "type", "types",
    "team", "teams", "device", "devices", "locally", "core", "part", "way", "thing",
];

fn extract_entities(text: &str) -> Vec<Entity> {
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entities: Vec<Entity> = Vec::new();

    let words: Vec<&str> = text.split_whitespace().collect();

    // --- Pass 1: multi-word entities (scan for consecutive Title Case runs) ---
    let mut i = 0;
    while i < words.len() && entities.len() < 20 {
        let token = words[i].trim_matches(STRIP_CHARS);

        // Check for URL or file first (single token)
        if is_url(token) || is_file(token) {
            if !token.is_empty() && !seen_names.contains(token) {
                let etype = if is_url(token) { EntityType::Url } else { EntityType::File };
                seen_names.insert(token.to_string());
                entities.push(Entity::new(token, etype, 0.8));
            }
            i += 1;
            continue;
        }

        // Try to grab a multi-word Title Case span (e.g. "Acme Corp", "New York")
        if is_title_case(token) && !is_stopword(token) {
            let start = i;
            let mut end = i + 1;
            while end < words.len() {
                let next = words[end].trim_matches(STRIP_CHARS);
                if is_title_case(next) && !next.is_empty() {
                    end += 1;
                } else {
                    break;
                }
            }

            if end - start >= 2 {
                // Multi-word entity
                let name: String = words[start..end]
                    .iter()
                    .map(|w| w.trim_matches(STRIP_CHARS))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !seen_names.contains(&name) {
                    let etype = classify_multi_word(&name);
                    seen_names.insert(name.clone());
                    // Also mark individual words as seen to avoid duplicates
                    for w in &words[start..end] {
                        seen_names.insert(w.trim_matches(STRIP_CHARS).to_string());
                    }
                    entities.push(Entity::new(&name, etype, 0.8));
                }
                i = end;
                continue;
            }
        }

        i += 1;
    }

    // --- Pass 2: single-token entities (skip already-seen) ---
    for raw_token in &words {
        if entities.len() >= 20 {
            break;
        }

        let token = raw_token.trim_matches(STRIP_CHARS);
        if token.is_empty() || seen_names.contains(token) {
            continue;
        }

        let entity_type = classify_token(token);
        if let Some(etype) = entity_type {
            seen_names.insert(token.to_string());
            entities.push(Entity::new(token, etype, 0.7));
        }
    }

    entities
}

fn is_url(token: &str) -> bool {
    token.starts_with("http://") || token.starts_with("https://") || token.starts_with("www.")
}

fn is_file(token: &str) -> bool {
    for ext in FILE_EXTENSIONS {
        if token.ends_with(ext) {
            return true;
        }
    }
    token.contains('/') && token.len() > 3
}

fn is_title_case(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() => true,
        _ => false,
    }
}

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// Classify a multi-word entity like "Acme Corp" or "Machine Learning".
fn classify_multi_word(name: &str) -> EntityType {
    let last_word = name.split_whitespace().last().unwrap_or("");

    // Check for org suffixes
    for suffix in ORG_SUFFIXES {
        if last_word.eq_ignore_ascii_case(suffix) {
            return EntityType::Organization;
        }
    }

    // Check if any word is a known tech term
    for word in name.split_whitespace() {
        if KNOWN_TECH.iter().any(|t| t.eq_ignore_ascii_case(word)) {
            return EntityType::Technology;
        }
    }

    // Default: if it looks like a proper noun phrase, treat as Person
    EntityType::Person
}

/// Return the `EntityType` for a single `token`, or `None` if it should be skipped.
fn classify_token(token: &str) -> Option<EntityType> {
    // 1. URL
    if is_url(token) {
        return Some(EntityType::Url);
    }

    // 2. File
    if is_file(token) {
        return Some(EntityType::File);
    }

    // 3. Known technology — takes priority over the Person heuristic
    if KNOWN_TECH.iter().any(|t| t.eq_ignore_ascii_case(token)) {
        return Some(EntityType::Technology);
    }

    // 4. Capitalized word that isn't a stopword → Person
    if is_title_case(token) && !is_stopword(token) {
        let mut chars = token.chars();
        chars.next(); // skip first
        let rest: String = chars.collect();
        if rest == rest.to_lowercase() {
            return Some(EntityType::Person);
        }
    }

    // 5. Concept: any token of length >= 4, but not a common verb/adj/functional word
    if token.len() >= 4 && !SKIP_WORDS.iter().any(|w| w.eq_ignore_ascii_case(token)) {
        return Some(EntityType::Concept);
    }

    None
}

// ---------------------------------------------------------------------------
// Triple extraction — pattern-based + co-occurrence fallback
// ---------------------------------------------------------------------------

/// Sentence-level pattern matching for typed predicates, with co-occurrence fallback.
fn extract_triples(text: &str, entities: &[Entity]) -> Vec<Triple> {
    let mut triples: Vec<Triple> = Vec::new();
    let text_lower = text.to_lowercase();

    // Build a lookup from lowercase entity name → entity index
    let entity_lookup: Vec<(String, usize)> = entities
        .iter()
        .enumerate()
        .map(|(idx, e)| (e.name.to_lowercase(), idx))
        .collect();

    // Try pattern-based extraction first
    let patterns: &[(&[&str], Predicate)] = &[
        // "X works at Y", "X working at Y", "X worked at Y"
        (&["works at", "working at", "worked at", "work at", "employed at", "employed by", "joined"], Predicate::WorksAt),
        // "X uses Y", "X using Y", "X built with Y"
        (&["uses", "using", "built with", "written in", "powered by", "implemented in", "runs on"], Predicate::Custom("uses".into())),
        // "X is a Y", "X is an Y"
        (&["is a ", "is an ", "are a ", "are an "], Predicate::IsA),
        // "X depends on Y", "X requires Y"
        (&["depends on", "requires", "needs", "relies on"], Predicate::DependsOn),
        // "X produces Y", "X generates Y", "X creates Y", "X building Y"
        (&["produces", "generates", "creates", "building", "built", "developing", "developed"], Predicate::Produces),
        // "X owns Y", "X created Y"
        (&["owns", "created", "founded", "started"], Predicate::Owns),
        // "X collaborates with Y", "X works with Y"
        (&["collaborates with", "works with", "partnered with", "teamed with"], Predicate::CollaboratesWith),
        // "X is part of Y", "X belongs to Y"
        (&["part of", "belongs to", "member of", "component of", "included in"], Predicate::PartOf),
        // "X references Y", "X mentions Y", "X links to Y"
        (&["references", "mentions", "links to", "points to", "refers to"], Predicate::References),
    ];

    for (keywords, predicate) in patterns {
        for keyword in *keywords {
            if !text_lower.contains(keyword) {
                continue;
            }
            // Find the keyword position in text_lower
            if let Some(kw_pos) = text_lower.find(keyword) {
                let before = &text_lower[..kw_pos];
                let after = &text_lower[kw_pos + keyword.len()..];

                // Find the closest entity in the text before and after the keyword
                let mut best_subj: Option<usize> = None;
                let mut best_subj_dist = usize::MAX;
                let mut best_obj: Option<usize> = None;
                let mut best_obj_dist = usize::MAX;

                for (ename, eidx) in &entity_lookup {
                    if let Some(pos) = before.rfind(ename.as_str()) {
                        let dist = before.len() - pos - ename.len();
                        if dist < best_subj_dist {
                            best_subj_dist = dist;
                            best_subj = Some(*eidx);
                        }
                    }
                    if let Some(pos) = after.find(ename.as_str()) {
                        if pos < best_obj_dist {
                            best_obj_dist = pos;
                            best_obj = Some(*eidx);
                        }
                    }
                }

                if let (Some(si), Some(oi)) = (best_subj, best_obj) {
                    if si != oi && triples.len() < 15 {
                        triples.push(Triple::new(
                            entities[si].id,
                            predicate.clone(),
                            entities[oi].id,
                            0.75,
                        ));
                    }
                }
            }
        }
    }

    // Co-occurrence fallback: fill remaining slots with RelatedTo (lower confidence)
    let window = entities.len().min(5);
    'outer: for i in 0..window {
        for j in (i + 1)..entities.len() {
            if triples.len() >= 15 {
                break 'outer;
            }
            // Skip if we already have a typed triple for this pair
            let already = triples.iter().any(|t| {
                (t.subject_id == entities[i].id && t.object_id == entities[j].id)
                    || (t.subject_id == entities[j].id && t.object_id == entities[i].id)
            });
            if already {
                continue;
            }
            triples.push(Triple::new(
                entities[i].id,
                Predicate::RelatedTo,
                entities[j].id,
                0.4,
            ));
        }
    }

    triples
}

// ---------------------------------------------------------------------------
// Content hash
// ---------------------------------------------------------------------------

fn hash_text(text: &str) -> String {
    format!("{:016x}", seahash::hash(text.as_bytes()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::TraceMindError;

    /// Open a pipeline backed by in-memory graph + temp-dir vector store.
    fn in_memory_pipeline() -> (IngestPipeline, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tm_ingest_{}", Uuid::new_v4()));
        let graph = GraphStore::open(":memory:").unwrap();
        let vector = VectorStore::open(dir.to_str().unwrap()).unwrap();
        let pipeline = IngestPipeline {
            graph,
            vector,
            embedder: Embedder::new_hash(),
            governance: GovernanceFilter::default(),
        };
        (pipeline, dir)
    }

    #[test]
    fn test_url_and_file_entities_extracted() {
        let (pipeline, dir) = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let result = pipeline
            .ingest("Visit https://example.com and read README.md", session_id)
            .expect("ingest should succeed");

        let has_url = result
            .entities
            .iter()
            .any(|e| e.entity_type == EntityType::Url);
        let has_file = result
            .entities
            .iter()
            .any(|e| e.entity_type == EntityType::File);

        assert!(has_url, "expected a Url entity; got: {:?}", result.entities);
        assert!(has_file, "expected a File entity; got: {:?}", result.entities);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_pii_email_rejected() {
        let (pipeline, dir) = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let result = pipeline.ingest("email: foo@bar.com", session_id);

        assert!(
            matches!(result, Err(TraceMindError::PiiDetected)),
            "expected PiiDetected, got: {:?}",
            result
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_concept_entities_extracted() {
        let (pipeline, dir) = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let text = "memory systems enable agents to recall information across sessions \
                    and build persistent knowledge over time";

        let result = pipeline.ingest(text, session_id).expect("ingest should succeed");

        assert!(
            !result.entities.is_empty(),
            "expected at least one entity from conceptual text"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_rust_classified_as_technology_not_person() {
        let entities = extract_entities("Rust is a fast language for systems programming");
        let rust = entities.iter().find(|e| e.name == "Rust");
        assert!(rust.is_some(), "expected Rust entity; got: {:?}", entities);
        assert_eq!(rust.unwrap().entity_type, EntityType::Technology);
    }

    #[test]
    fn test_multi_word_entity_extraction() {
        let entities = extract_entities("Aaditya Srivathsan works at Acme Corp on TraceMind");
        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("Aaditya") && n.contains("Srivathsan")),
            "expected multi-word entity 'Aaditya Srivathsan'; got: {:?}", names
        );
        let acme = entities.iter().find(|e| e.name.contains("Acme"));
        assert!(acme.is_some(), "expected Acme Corp entity; got: {:?}", names);
        assert_eq!(acme.unwrap().entity_type, EntityType::Organization);
    }

    #[test]
    fn test_typed_predicate_works_at() {
        let text = "Aaditya works at Anthropic on AI safety";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        let has_works_at = triples.iter().any(|t| t.predicate == Predicate::WorksAt);
        assert!(
            has_works_at,
            "expected WorksAt predicate; got: {:?}",
            triples.iter().map(|t| &t.predicate).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_typed_predicate_uses() {
        let text = "TraceMind uses Rust for performance";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        let has_uses = triples.iter().any(|t| t.predicate == Predicate::Custom("uses".into()));
        assert!(
            has_uses,
            "expected 'uses' predicate; got: {:?}",
            triples.iter().map(|t| &t.predicate).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_co_occurrence_skips_already_typed_pairs() {
        let text = "Aaditya works at Google on search";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        // Find the pair that has WorksAt
        let works_at = triples.iter().find(|t| t.predicate == Predicate::WorksAt);
        if let Some(wa) = works_at {
            // There should be no RelatedTo for the same pair
            let dup = triples.iter().any(|t| {
                t.predicate == Predicate::RelatedTo
                    && ((t.subject_id == wa.subject_id && t.object_id == wa.object_id)
                        || (t.subject_id == wa.object_id && t.object_id == wa.subject_id))
            });
            assert!(!dup, "co-occurrence should not duplicate typed pairs");
        }
    }
}
