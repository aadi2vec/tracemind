use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A named entity in the knowledge graph (person, concept, project, file, URL…)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: EntityType,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity {
    pub fn new(name: impl Into<String>, entity_type: EntityType, confidence: f64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            entity_type,
            confidence: confidence.clamp(0.0, 1.0),
            source_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn decay(&mut self, factor: f64) {
        self.confidence = (self.confidence * factor).max(0.0);
        self.updated_at = Utc::now();
    }

    pub fn reinforce(&mut self, amount: f64) {
        self.confidence = (self.confidence + amount).min(1.0);
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Organization,
    Project,
    File,
    Url,
    Concept,
    Technology,
    Decision,
    Event,
    /// LM-5c — first-class daily-note memory. One per local date,
    /// auto-named `YYYY-MM-DD`, populated by `tracemind today`.
    /// Existence is idempotent: re-running `today` upserts.
    DailyNote,
    /// LM-5d — auto-generated Map of Content for a cluster/community.
    /// Name = c-TF-IDF label (or fallback entity-type label when
    /// `tm-cluster` hasn't filled in HDBSCAN labels yet). Backlinks
    /// to all member memories; refreshed nightly by `consolidate`.
    MapOfContent,
    Custom(String),
}

/// LM-15 — ontological domain typing.
///
/// Coarse-grained class for an entity that lets bridge proposals
/// reject cross-domain matches at the type level (the "Harry Potter
/// the wizard ↔ Harry Potter the historical figure" problem).
///
/// The heuristic classifier in `tm-graph::ontology::classify` maps
/// any `(EntityType, name)` pair to a default domain. An SML pass
/// (LM-6/LM-8 dependent) can later override this for a specific
/// entity by storing the explicit value alongside `entity_type`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OntologicalDomain {
    /// A real-world person (Pat Grady, Andrej Karpathy).
    PersonReal,
    /// A fictional or hypothetical person (Harry Potter the wizard).
    PersonFictional,
    /// A real geographic place (Alcatraz, San Francisco).
    LocationReal,
    /// A fictional place (Hogwarts).
    LocationFictional,
    /// A real organization (Sequoia, OpenAI).
    OrganizationReal,
    /// A real-world event (the 2008 financial crisis).
    EventReal,
    /// A fictional event (the Battle of Hogwarts).
    EventFictional,
    /// A concept / idea / abstraction (entropy, RAG).
    Concept,
    /// A technology / product / library (Rust, SQLite).
    Technology,
    /// A file or URL on the user's machine / the web.
    Artifact,
    /// A user decision (auto-classified `EntityType::Decision`).
    Decision,
    /// The user's own daily-note memory.
    DailyNote,
    /// MOC entities are their own domain so they don't bridge.
    MapOfContent,
    /// Unknown — used when the classifier abstains. Bridges across
    /// `Unknown` are permitted (we don't want to over-block).
    Unknown,
}

impl OntologicalDomain {
    /// Two domains are *compatible* for bridge proposals when they are
    /// the same, when either is `Unknown`, or when they are both within
    /// a logically-related pair (e.g. `Concept` ↔ `Technology`).
    ///
    /// `PersonReal` ↔ `PersonFictional` and `LocationReal` ↔
    /// `LocationFictional` are the canonical *incompatible* pairs —
    /// these are the Harry Potter problem.
    pub fn is_compatible_with(self, other: OntologicalDomain) -> bool {
        use OntologicalDomain::*;
        if self == other {
            return true;
        }
        if matches!(self, Unknown) || matches!(other, Unknown) {
            return true;
        }
        match (self, other) {
            // Real/fictional pairs of the same shape are explicitly
            // incompatible — the whole point of this enum.
            (PersonReal, PersonFictional) | (PersonFictional, PersonReal) => false,
            (LocationReal, LocationFictional) | (LocationFictional, LocationReal) => false,
            (EventReal, EventFictional) | (EventFictional, EventReal) => false,
            // Concept ↔ Technology is fine; both abstract.
            (Concept, Technology) | (Technology, Concept) => true,
            // MOC bridges to nothing else; it's a routing node.
            (MapOfContent, _) | (_, MapOfContent) => false,
            // Daily notes only bridge to daily notes.
            (DailyNote, _) | (_, DailyNote) => false,
            // Everything else: incompatible by default. This is
            // conservative — start strict, relax with feedback.
            _ => false,
        }
    }
}

impl std::fmt::Display for OntologicalDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OntologicalDomain::PersonReal => "person.real",
            OntologicalDomain::PersonFictional => "person.fictional",
            OntologicalDomain::LocationReal => "location.real",
            OntologicalDomain::LocationFictional => "location.fictional",
            OntologicalDomain::OrganizationReal => "organization.real",
            OntologicalDomain::EventReal => "event.real",
            OntologicalDomain::EventFictional => "event.fictional",
            OntologicalDomain::Concept => "concept",
            OntologicalDomain::Technology => "technology",
            OntologicalDomain::Artifact => "artifact",
            OntologicalDomain::Decision => "decision",
            OntologicalDomain::DailyNote => "daily_note",
            OntologicalDomain::MapOfContent => "map_of_content",
            OntologicalDomain::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntityType::Custom(s) => write!(f, "{}", s),
            other => write!(f, "{:?}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_confidence_clamps() {
        let e = Entity::new("test", EntityType::Concept, 1.5);
        assert_eq!(e.confidence, 1.0);
        let e = Entity::new("test", EntityType::Concept, -0.1);
        assert_eq!(e.confidence, 0.0);
    }

    #[test]
    fn entity_decay_works() {
        let mut e = Entity::new("test", EntityType::Concept, 1.0);
        e.decay(0.9);
        assert!((e.confidence - 0.9).abs() < 1e-10);
    }
}
