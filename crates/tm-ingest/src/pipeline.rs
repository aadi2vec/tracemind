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
    pub trace_id: Uuid,
    pub entities: Vec<Entity>,
    pub triples: Vec<Triple>,
    pub content_hash: String,
}

impl IngestPipeline {
    /// Open graph store at `db_path`, vector store at `db_path` + ".vec",
    /// and initialise embedder and default governance filter.
    pub fn open(db_path: &str) -> Result<Self> {
        let graph = GraphStore::open(db_path)?;
        let vec_path = format!("{}.vec", db_path);
        let vector = VectorStore::open(&vec_path)?;
        let embedder = Embedder::new();
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
    /// 3. Extract entities heuristically.
    /// 4. Upsert each entity to the graph store and vector store.
    /// 5. Extract triples from co-occurring entities.
    /// 6. Upsert each triple to the graph store.
    /// 7. Return an `IngestResult`.
    pub fn ingest(&self, text: &str, session_id: Uuid) -> Result<IngestResult> {
        // 1. Governance check.
        self.governance.check(text, 1.0)?;

        // 2. Hash.
        let content_hash = hash_text(text);

        // 3. Extract entities.
        let entities = extract_entities(text);

        // 4. Upsert entities to graph + vector stores.
        for entity in &entities {
            self.graph.upsert_entity(entity)?;
            let embedding = self.embedder.embed(&entity.name);
            self.vector.upsert(entity.id, &embedding)?;
        }

        // 5. Extract triples.
        let triples = extract_triples(&entities);

        // 6. Upsert triples to graph.
        for triple in &triples {
            self.graph.upsert_triple(triple)?;
        }

        // 7. Build trace record (stored in-memory for now; callers may persist it).
        let mut trace = Trace::new(session_id, TraceEventType::Ingest, &content_hash);
        trace.entities_extracted = entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = triples.iter().map(|t| t.id).collect();

        Ok(IngestResult {
            trace_id: trace.id,
            entities,
            triples,
            content_hash,
        })
    }
}

// ---------------------------------------------------------------------------
// Heuristic NER
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "The", "A", "An", "In", "On", "At", "To", "For", "Of", "And", "Or", "But", "Is", "Was",
    "Are", "Were", "Be", "Been", "It", "Its", "This", "That", "With", "From", "By", "As", "Up",
    "Out", "If", "So", "No",
];

const FILE_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".js", ".ts", ".go", ".md", ".txt", ".json", ".toml",
];

const STRIP_CHARS: &[char] = &[',', '.', '!', '?', ';', ':', '"', '\'', '(', ')'];

fn extract_entities(text: &str) -> Vec<Entity> {
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entities: Vec<Entity> = Vec::new();

    for raw_token in text.split_whitespace() {
        if entities.len() >= 20 {
            break;
        }

        // Strip leading and trailing punctuation.
        let token = raw_token.trim_matches(STRIP_CHARS);
        if token.is_empty() {
            continue;
        }

        // Deduplicate by name.
        if seen_names.contains(token) {
            continue;
        }

        let entity_type = classify_token(token);

        // For Concept tokens we require length >= 4; the classifier already
        // handles that, but we double-check here for clarity.
        if entity_type.is_none() {
            continue;
        }
        let entity_type = entity_type.unwrap();

        seen_names.insert(token.to_string());
        entities.push(Entity::new(token, entity_type, 0.7));
    }

    entities
}

/// Return the `EntityType` for `token`, or `None` if it should be skipped.
///
/// Rules are applied in order; the first match wins.
fn classify_token(token: &str) -> Option<EntityType> {
    // 1. URL
    if token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("www.")
    {
        return Some(EntityType::Url);
    }

    // 2. File by extension
    for ext in FILE_EXTENSIONS {
        if token.ends_with(ext) {
            return Some(EntityType::File);
        }
    }

    // 3. File by path-like slash (not a URL, length > 3)
    if token.contains('/') && token.len() > 3 {
        return Some(EntityType::File);
    }

    // 4. Capitalized word that isn't a stopword → Person (heuristic)
    if token.len() >= 2 {
        let mut chars = token.chars();
        if let Some(first) = chars.next() {
            let rest: String = chars.collect();
            if first.is_uppercase() && rest == rest.to_lowercase() {
                // It's capitalised (Title Case).
                if !STOPWORDS.contains(&token) {
                    return Some(EntityType::Person);
                }
            }
        }
    }

    // 5. Concept: any token of length >= 4
    if token.len() >= 4 {
        return Some(EntityType::Concept);
    }

    None
}

// ---------------------------------------------------------------------------
// Triple extraction
// ---------------------------------------------------------------------------

fn extract_triples(entities: &[Entity]) -> Vec<Triple> {
    let mut triples: Vec<Triple> = Vec::new();

    let window = entities.len().min(5);

    'outer: for i in 0..window {
        for j in (i + 1)..entities.len() {
            if triples.len() >= 10 {
                break 'outer;
            }
            triples.push(Triple::new(
                entities[i].id,
                Predicate::RelatedTo,
                entities[j].id,
                0.5,
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

    /// Open a pipeline backed by in-memory SQLite databases.
    fn in_memory_pipeline() -> IngestPipeline {
        let graph = GraphStore::open(":memory:").unwrap();
        let vector = VectorStore::open(":memory:").unwrap();
        IngestPipeline {
            graph,
            vector,
            embedder: Embedder::new(),
            governance: GovernanceFilter::default(),
        }
    }

    #[test]
    fn test_url_and_file_entities_extracted() {
        let pipeline = in_memory_pipeline();
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
    }

    #[test]
    fn test_pii_email_rejected() {
        let pipeline = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let result = pipeline.ingest("email: foo@bar.com", session_id);

        assert!(
            matches!(result, Err(TraceMindError::PiiDetected)),
            "expected PiiDetected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_concept_entities_extracted() {
        let pipeline = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let text = "memory systems enable agents to recall information across sessions \
                    and build persistent knowledge over time";

        let result = pipeline.ingest(text, session_id).expect("ingest should succeed");

        assert!(
            !result.entities.is_empty(),
            "expected at least one entity from conceptual text"
        );
    }
}
