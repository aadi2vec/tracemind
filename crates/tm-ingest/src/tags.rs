//! LM-5b — auto-tags from heuristic hashtags + entity-type fallback.
//!
//! Once `tm-cluster` (CLU-6) lands, this module's `cluster_label`
//! function will be replaced with the real HDBSCAN c-TF-IDF labels.
//! Until then, the entity-type fallback gives Tauri/MCP a deterministic
//! tag surface without blocking on the clustering pipeline.

use tm_types::{Entity, EntityType};

/// Strip the leading `#` from `tag` and lowercase the rest. Returns
/// `None` for empty inputs or anything that doesn't look like a word.
fn normalize_tag(tag: &str) -> Option<String> {
    let trimmed = tag.trim_start_matches('#').trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.chars().next()?.is_alphanumeric() {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_lowercase())
    }
}

/// Extract literal `#hashtag` tokens from `text`, in document order,
/// deduplicated (case-insensitively).
pub fn extract_hashtags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch != '#' {
            continue;
        }
        // Tag must be preceded by a word boundary so we don't grab
        // the "#" in URLs like `#section-2` on a fragment, but we do
        // want to grab "#rust" at the start of a line.
        if i > 0 {
            let prev = text[..i].chars().last();
            if matches!(prev, Some(c) if c.is_alphanumeric()) {
                continue;
            }
        }
        // Consume the tag body.
        let start = i + 1;
        let mut end = start;
        while let Some(&(j, c)) = chars.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                end = j + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        if end > start {
            if let Some(tag) = normalize_tag(&text[start..end]) {
                if seen.insert(tag.clone()) {
                    out.push(tag);
                }
            }
        }
    }
    out
}

/// Best-effort tag label for an entity, used as a stand-in until
/// `tm-cluster` populates HDBSCAN c-TF-IDF labels.
pub fn entity_type_tag(entity_type: &EntityType) -> String {
    match entity_type {
        EntityType::Person => "people".to_string(),
        EntityType::Organization => "orgs".to_string(),
        EntityType::Project => "projects".to_string(),
        EntityType::File => "files".to_string(),
        EntityType::Url => "urls".to_string(),
        EntityType::Concept => "concepts".to_string(),
        EntityType::Technology => "tech".to_string(),
        EntityType::Decision => "decisions".to_string(),
        EntityType::Event => "events".to_string(),
        EntityType::DailyNote => "daily".to_string(),
        EntityType::MapOfContent => "moc".to_string(),
        EntityType::Custom(s) => s.to_lowercase(),
    }
}

/// Tag set for a memory: hashtags from text, plus the entity-type
/// fallback label for every entity touched, deduplicated.
pub fn tags_for_memory(text: &str, entities: &[Entity]) -> Vec<String> {
    let mut tags = extract_hashtags(text);
    let mut seen: std::collections::HashSet<String> = tags.iter().cloned().collect();
    for ent in entities {
        let label = entity_type_tag(&ent.entity_type);
        if seen.insert(label.clone()) {
            tags.push(label);
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::Entity;

    #[test]
    fn hashtag_extraction_basic() {
        let tags = extract_hashtags("today I shipped #rust and #sqlite work");
        assert_eq!(tags, vec!["rust".to_string(), "sqlite".to_string()]);
    }

    #[test]
    fn hashtag_extraction_dedupes_case_insensitively() {
        let tags = extract_hashtags("#Rust then #rust then #RUST");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0], "rust");
    }

    #[test]
    fn hashtag_extraction_skips_url_fragments() {
        // `#section` here follows alphanumeric (the "html") so it
        // should not be picked up as a hashtag.
        let tags = extract_hashtags("see docs.html#section for context");
        assert!(tags.is_empty());
    }

    #[test]
    fn hashtag_extraction_keeps_dashes_and_underscores() {
        let tags = extract_hashtags("#design-partner #wme_v2");
        assert_eq!(tags, vec!["design-partner".to_string(), "wme_v2".to_string()]);
    }

    #[test]
    fn entity_type_tag_for_known_variants() {
        assert_eq!(entity_type_tag(&EntityType::Person), "people");
        assert_eq!(entity_type_tag(&EntityType::Technology), "tech");
        assert_eq!(
            entity_type_tag(&EntityType::Custom("research".into())),
            "research"
        );
    }

    #[test]
    fn tags_for_memory_combines_hashtags_and_entities() {
        let entities = vec![
            Entity::new("Aaditya", EntityType::Person, 0.9),
            Entity::new("TraceMind", EntityType::Project, 0.9),
        ];
        let tags = tags_for_memory("morning standup #planning", &entities);
        assert!(tags.contains(&"planning".to_string()));
        assert!(tags.contains(&"people".to_string()));
        assert!(tags.contains(&"projects".to_string()));
    }

    #[test]
    fn tags_for_memory_is_idempotent_in_dedup() {
        let entities = vec![
            Entity::new("Foo", EntityType::Concept, 0.5),
            Entity::new("Bar", EntityType::Concept, 0.5),
        ];
        let tags = tags_for_memory("no hashtags here", &entities);
        assert_eq!(tags.iter().filter(|t| *t == "concepts").count(), 1);
    }
}
