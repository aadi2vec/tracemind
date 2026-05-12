//! LM-15 — heuristic ontological-domain classifier.
//!
//! Maps `(EntityType, name)` → [`OntologicalDomain`]. Used to gate
//! bridge proposals across disjoint domains (Harry Potter the wizard
//! ↔ Harry Potter the historical figure is the canonical example).
//!
//! This is **only the heuristic floor**. When an SML pass is wired
//! through `TripleWorker` (LM-8 / LM-6) it can override per-entity
//! by writing the explicit domain into the entity's properties JSON.

use tm_types::{EntityType, OntologicalDomain};

/// A short list of obviously-fictional name fragments that nudge an
/// otherwise ambiguous classification toward `*Fictional`. Kept
/// intentionally small and surface-level — the SML override is where
/// nuance comes from.
const FICTIONAL_HINTS: &[&str] = &[
    "harry potter",
    "hogwarts",
    "voldemort",
    "hermione",
    "dumbledore",
    "frodo",
    "gandalf",
    "middle-earth",
    "narnia",
    "westeros",
    "gotham",
    "wakanda",
    "hyrule",
    "azkaban",
    "mordor",
];

/// Map an entity to its default ontological domain.
///
/// The classifier is deliberately conservative: when the type is
/// abstract (`Concept`, `Technology`) we never invent a fictional
/// flavor; when the name contains a fictional fragment we flip the
/// real/fictional flag.
pub fn classify(entity_type: &EntityType, name: &str) -> OntologicalDomain {
    let lower = name.to_lowercase();
    let is_fictional = FICTIONAL_HINTS.iter().any(|h| lower.contains(h));

    match entity_type {
        EntityType::Person => {
            if is_fictional {
                OntologicalDomain::PersonFictional
            } else {
                OntologicalDomain::PersonReal
            }
        }
        EntityType::Organization => OntologicalDomain::OrganizationReal,
        EntityType::Project => OntologicalDomain::Concept,
        EntityType::File | EntityType::Url => OntologicalDomain::Artifact,
        EntityType::Concept => OntologicalDomain::Concept,
        EntityType::Technology => OntologicalDomain::Technology,
        EntityType::Decision => OntologicalDomain::Decision,
        EntityType::Event => {
            if is_fictional {
                OntologicalDomain::EventFictional
            } else {
                OntologicalDomain::EventReal
            }
        }
        EntityType::DailyNote => OntologicalDomain::DailyNote,
        EntityType::MapOfContent => OntologicalDomain::MapOfContent,
        EntityType::Custom(s) => {
            // Best-effort guess from the custom string.
            let s = s.to_lowercase();
            if s.contains("location") || s.contains("place") {
                if is_fictional {
                    OntologicalDomain::LocationFictional
                } else {
                    OntologicalDomain::LocationReal
                }
            } else if s.contains("person") {
                if is_fictional {
                    OntologicalDomain::PersonFictional
                } else {
                    OntologicalDomain::PersonReal
                }
            } else {
                OntologicalDomain::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_person_defaults_to_person_real() {
        assert_eq!(
            classify(&EntityType::Person, "Pat Grady"),
            OntologicalDomain::PersonReal
        );
    }

    #[test]
    fn fictional_person_name_flips_domain() {
        assert_eq!(
            classify(&EntityType::Person, "Harry Potter"),
            OntologicalDomain::PersonFictional
        );
    }

    #[test]
    fn fictional_event_name_flips_domain() {
        assert_eq!(
            classify(&EntityType::Event, "Battle of Hogwarts"),
            OntologicalDomain::EventFictional
        );
    }

    #[test]
    fn technology_classifies_as_technology() {
        assert_eq!(
            classify(&EntityType::Technology, "Rust"),
            OntologicalDomain::Technology
        );
    }

    #[test]
    fn moc_classifies_as_moc() {
        assert_eq!(
            classify(&EntityType::MapOfContent, "AI Research"),
            OntologicalDomain::MapOfContent
        );
    }

    #[test]
    fn real_and_fictional_persons_are_incompatible() {
        assert!(!OntologicalDomain::PersonReal
            .is_compatible_with(OntologicalDomain::PersonFictional));
    }

    #[test]
    fn concept_and_technology_are_compatible() {
        assert!(OntologicalDomain::Concept
            .is_compatible_with(OntologicalDomain::Technology));
    }

    #[test]
    fn unknown_is_compatible_with_anything() {
        assert!(OntologicalDomain::Unknown
            .is_compatible_with(OntologicalDomain::PersonFictional));
    }

    #[test]
    fn alcatraz_vs_hogwarts_is_blocked_at_domain_level() {
        // Alcatraz → Custom("location")-equivalent → LocationReal.
        // Hogwarts → contains fictional hint → LocationFictional.
        let alcatraz = classify(&EntityType::Custom("location".into()), "Alcatraz");
        let hogwarts = classify(&EntityType::Custom("location".into()), "Hogwarts");
        assert_eq!(alcatraz, OntologicalDomain::LocationReal);
        assert_eq!(hogwarts, OntologicalDomain::LocationFictional);
        assert!(!alcatraz.is_compatible_with(hogwarts));
    }
}
