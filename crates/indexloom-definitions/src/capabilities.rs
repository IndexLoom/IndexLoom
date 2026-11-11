//! Read-only structured Cardigann capabilities derived from original source data.
//! Genres are search parameters, not a fabricated list of site genres.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_yaml::{Mapping, Value};

use crate::{IndexerDefinition, SourceProject};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerCapabilities {
    pub description: String,
    pub language: String,
    pub source: &'static str,
    pub search_modes: Vec<SearchMode>,
    pub category_mappings: Vec<CategoryMapping>,
    pub categories: Vec<String>,
    pub genre_search: bool,
    pub raw_search: bool,
    pub authentication: bool,
    pub captcha: bool,
    pub download_handler: bool,
    pub search_paths: usize,
    pub alternate_urls: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMode {
    pub name: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryMapping {
    pub id: String,
    pub category: String,
    pub description: Option<String>,
    pub default: bool,
}

fn entry<'a>(mapping: &'a Mapping, name: &str) -> Option<&'a Value> {
    mapping.get(Value::String(name.to_owned()))
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

#[must_use]
pub fn summarize(definition: &IndexerDefinition) -> IndexerCapabilities {
    let capabilities = definition.spec.implementation.capabilities.as_mapping();
    let mut search_modes = Vec::new();
    let mut categories = BTreeSet::new();
    let mut category_mappings = Vec::new();

    if let Some(modes) = capabilities
        .and_then(|caps| entry(caps, "modes"))
        .and_then(Value::as_mapping)
    {
        for (name, fields) in modes {
            if let Some(name) = scalar(name) {
                let parameters = fields
                    .as_sequence()
                    .map_or_else(Vec::new, |seq| seq.iter().filter_map(scalar).collect());
                search_modes.push(SearchMode { name, parameters });
            }
        }
    }
    if let Some(mapping) = capabilities
        .and_then(|caps| entry(caps, "categorymappings"))
        .and_then(Value::as_sequence)
    {
        for value in mapping {
            let Some(value) = value.as_mapping() else {
                continue;
            };
            let Some(id) = entry(value, "id").and_then(scalar) else {
                continue;
            };
            let Some(category) = entry(value, "cat").and_then(scalar) else {
                continue;
            };
            categories.insert(category.clone());
            category_mappings.push(CategoryMapping {
                id,
                category,
                description: entry(value, "desc").and_then(scalar),
                default: matches!(entry(value, "default"), Some(Value::Bool(true))),
            });
        }
    }
    // Some source definitions use category maps rather than categorymappings.
    if let Some(mapping) = capabilities
        .and_then(|caps| entry(caps, "categories"))
        .and_then(Value::as_mapping)
    {
        for key in mapping.keys() {
            if let Some(category) = scalar(key) {
                categories.insert(category);
            }
        }
    }
    let search = definition.spec.implementation.search.as_mapping();
    let search_paths = search
        .and_then(|search| entry(search, "paths"))
        .and_then(Value::as_sequence)
        .map_or(0, Vec::len);
    let authentication = definition.spec.implementation.authentication.as_ref();
    let captcha = authentication
        .and_then(Value::as_mapping)
        .is_some_and(|auth| entry(auth, "captcha").is_some());
    let source = match definition.provenance.source.project {
        SourceProject::Jackett => "jackett",
        SourceProject::ProwlarrIndexers => "prowlarr-indexers",
    };
    let genre_search = search_modes
        .iter()
        .any(|mode| mode.parameters.iter().any(|p| p == "genre"));
    IndexerCapabilities {
        description: definition.metadata.description.clone(),
        language: definition.metadata.language.clone(),
        source,
        search_modes,
        category_mappings,
        categories: categories.into_iter().collect(),
        genre_search,
        raw_search: matches!(
            capabilities.and_then(|caps| entry(caps, "allowrawsearch")),
            Some(Value::Bool(true))
        ),
        authentication: authentication.is_some(),
        captcha,
        download_handler: definition.spec.implementation.download.is_some(),
        search_paths,
        alternate_urls: definition.spec.base_urls.len().saturating_sub(1)
            + definition.spec.legacy_urls.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::summarize;
    use crate::parse_yaml;

    #[test]
    fn preserves_source_category_ids_and_genre_query_capability() {
        let yaml = r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata: { id: example, name: Example, description: Test indexer, language: en }
provenance:
  generatedBy: indexloom-importer/v1
  source: { project: jackett, repository: "https://github.com/Jackett/Jackett",
    revision: "0000000000000000000000000000000000000000",
    path: "src/Jackett.Common/Definitions/example.yml", sourceId: example,
    license: GPL-2.0-only,
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000",
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111" }
spec:
  protocol: torrent
  privacy: public
  baseUrls: ["https://example.invalid/"]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities:
      modes: { search: [q], movie-search: [q, genre] }
      categorymappings:
        - { id: 0_0, cat: TV/Anime, desc: Anime, default: true }
        - { id: 123, cat: Movies/HD }
      allowrawsearch: true
    settings: []
    search: { paths: [{ path: /search }, { path: /backup }], rows: { selector: .result }, fields: {} }
"#;
        let definition = parse_yaml(yaml).expect("valid YAML definition");
        let caps = summarize(&definition);
        assert_eq!(caps.category_mappings[0].id, "0_0");
        assert_eq!(caps.categories, ["Movies/HD", "TV/Anime"]);
        assert!(caps.category_mappings[0].default);
        assert_eq!(caps.search_modes.len(), 2);
        assert!(caps.genre_search);
        assert!(caps.raw_search);
        assert_eq!(caps.search_paths, 2);
    }
}
