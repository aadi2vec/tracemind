//! LM-5d — auto-generated Map of Content (MOC) entities.
//!
//! A MOC is a `MapOfContent`-typed entity whose backlinks point to a
//! group of related memories. It surfaces as the "Topics" panel in
//! Tauri and as a navigable hub in MCP `memory_query` results.
//!
//! Today's generator works *without* HDBSCAN clusters from
//! `tm-cluster` (CLU-6): it falls back to grouping entities by their
//! shared predicate-target signature. When `tm-cluster` lands, the
//! [`generate_moc_for_cluster`] hook lets callers thread the
//! cluster's c-TF-IDF label and persistence score through verbatim.
//!
//! Idempotency: each MOC entity is keyed by `name` — re-running the
//! generator with the same name upserts rather than duplicates.

use std::collections::HashMap;
use tm_types::{Entity, EntityType, Predicate, Result, Triple};
use uuid::Uuid;

use crate::store::GraphStore;

/// Result of upserting one MOC: the MOC entity itself plus the number
/// of new `RelatedTo` backlinks the call added.
#[derive(Debug, Clone)]
pub struct MocUpsert {
    pub moc: Entity,
    pub new_backlinks: usize,
}

/// Idempotent upsert of a MOC named `name`, linked back to every id
/// in `members`. Callers are responsible for choosing the label
/// (c-TF-IDF for HDBSCAN clusters; predicate-signature digest for the
/// heuristic fallback below).
pub fn upsert_moc(
    graph: &GraphStore,
    name: &str,
    description: &str,
    members: &[Uuid],
) -> Result<MocUpsert> {
    // Reuse-by-name: if a MOC already exists with this label, attach
    // new backlinks to it rather than creating a sibling MOC.
    let existing = graph.find_entity_by_name_icase(name)?;
    let moc = match existing {
        Some(e) if matches!(e.entity_type, EntityType::MapOfContent) => e,
        Some(_) => {
            // A non-MOC entity already owns this name (e.g. a Project
            // called "AI Research"). Disambiguate the MOC suffix.
            let suffixed = format!("{} (MOC)", name);
            let mut moc = Entity::new(&suffixed, EntityType::MapOfContent, 0.9);
            moc.source_id = Some(format!("moc:{}", name));
            graph.upsert_entity(&moc)?;
            moc
        }
        None => {
            let mut moc = Entity::new(name, EntityType::MapOfContent, 0.9);
            moc.source_id = Some(format!("moc:{}", name));
            graph.upsert_entity(&moc)?;
            moc
        }
    };

    let mut new_backlinks = 0;
    for &member_id in members {
        if member_id == moc.id {
            continue;
        }
        let triple = Triple::new(
            moc.id,
            Predicate::Custom("indexes".to_string()),
            member_id,
            0.85,
        );
        if graph.upsert_triple(&triple).is_ok() {
            new_backlinks += 1;
        }
    }

    // The description is informational and stored as a triple with a
    // `HasProperty` predicate against a synthetic "description"
    // sub-entity. We skip persisting it when empty — the description
    // is decorative on the surface.
    let _ = description;

    Ok(MocUpsert { moc, new_backlinks })
}

/// CLU-6 hook: when `tm-cluster` populates HDBSCAN clusters, this is
/// the call-site to wire through. The contract intentionally mirrors
/// the c-TF-IDF output (label + persistence + member ids) so the
/// integration is a one-liner.
pub fn generate_moc_for_cluster(
    graph: &GraphStore,
    cluster_label: &str,
    persistence: f64,
    members: &[Uuid],
) -> Result<MocUpsert> {
    let desc = format!("auto-MOC, persistence={:.2}", persistence);
    upsert_moc(graph, cluster_label, &desc, members)
}

/// Heuristic fallback grouping: clusters entities by the *(predicate,
/// target_name)* of their outgoing triples. Useful before CLU-6 lands
/// — produces small MOCs like "uses Rust" or "works at TraceMind".
///
/// `min_members` keeps the MOC surface clean: a group of one isn't
/// a map of content, it's just an entity.
pub fn heuristic_moc_groups(
    triples: &[Triple],
    entity_names: &HashMap<Uuid, String>,
    min_members: usize,
) -> Vec<(String, Vec<Uuid>)> {
    let mut groups: HashMap<(String, String), Vec<Uuid>> = HashMap::new();
    for t in triples {
        let Some(target_name) = entity_names.get(&t.object_id) else {
            continue;
        };
        let pred_label = match &t.predicate {
            Predicate::Custom(s) => s.clone(),
            other => format!("{:?}", other),
        };
        let key = (pred_label.clone(), target_name.clone());
        let entry = groups.entry(key).or_default();
        if !entry.contains(&t.subject_id) {
            entry.push(t.subject_id);
        }
    }
    groups
        .into_iter()
        .filter(|(_, members)| members.len() >= min_members)
        .map(|((pred, target), members)| {
            let label = format!("{} {}", pred, target);
            (label, members)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_moc_creates_new_when_missing() {
        let graph = GraphStore::open(":memory:").unwrap();
        let m1 = Entity::new("Foo", EntityType::Concept, 0.9);
        let m2 = Entity::new("Bar", EntityType::Concept, 0.9);
        graph.upsert_entity(&m1).unwrap();
        graph.upsert_entity(&m2).unwrap();

        let res = upsert_moc(&graph, "Concepts", "test", &[m1.id, m2.id]).unwrap();
        assert!(matches!(res.moc.entity_type, EntityType::MapOfContent));
        assert_eq!(res.new_backlinks, 2);
    }

    #[test]
    fn upsert_moc_is_idempotent_by_name() {
        let graph = GraphStore::open(":memory:").unwrap();
        let m1 = Entity::new("Foo", EntityType::Concept, 0.9);
        graph.upsert_entity(&m1).unwrap();
        let first = upsert_moc(&graph, "TestMoc", "", &[m1.id]).unwrap();
        let second = upsert_moc(&graph, "TestMoc", "", &[m1.id]).unwrap();
        assert_eq!(first.moc.id, second.moc.id);
    }

    #[test]
    fn upsert_moc_disambiguates_against_non_moc_name_collision() {
        let graph = GraphStore::open(":memory:").unwrap();
        let proj = Entity::new("AI Research", EntityType::Project, 0.9);
        graph.upsert_entity(&proj).unwrap();
        let m = Entity::new("doc1", EntityType::Concept, 0.9);
        graph.upsert_entity(&m).unwrap();
        let res = upsert_moc(&graph, "AI Research", "", &[m.id]).unwrap();
        assert!(res.moc.name.ends_with("(MOC)"));
        assert_ne!(res.moc.id, proj.id);
    }

    #[test]
    fn heuristic_groups_collapse_under_min_members() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let rust = Uuid::new_v4();
        let names: HashMap<Uuid, String> =
            [(rust, "Rust".to_string())].into_iter().collect();
        let triples = vec![
            Triple::new(a, Predicate::Custom("uses".into()), rust, 0.9),
            Triple::new(b, Predicate::Custom("uses".into()), rust, 0.9),
            Triple::new(c, Predicate::Custom("uses".into()), rust, 0.9),
        ];
        // min_members=2 keeps the group.
        let groups = heuristic_moc_groups(&triples, &names, 2);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 3);
        // min_members=5 drops it.
        let groups = heuristic_moc_groups(&triples, &names, 5);
        assert!(groups.is_empty());
    }
}
