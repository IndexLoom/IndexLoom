//! Per-search template scope; cloned settings never escape their indexer boundary.
use super::filters::json_template_value;
use super::{BTreeMap, JsonValue, OffsetDateTime, SearchParameters, TemplateValue, set_pagination};
use std::collections::HashMap;
#[derive(Clone)]
pub(super) struct RenderContext {
    pub(super) config: HashMap<String, TemplateValue>,
    pub(super) parameters: SearchParameters,
    pub(super) categories: Vec<String>,
    pub(super) limit: usize,
    pub(super) offset: usize,
    pub(super) skip_andmatch: bool,
}

impl std::fmt::Debug for RenderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderContext").finish_non_exhaustive()
    }
}

impl RenderContext {
    pub(super) fn new(
        values: BTreeMap<String, JsonValue>,
        secrets: BTreeMap<String, String>,
    ) -> Self {
        let mut config = values
            .into_iter()
            .map(|(name, value)| (name, json_template_value(value)))
            .collect::<HashMap<_, _>>();
        config.extend(
            secrets
                .into_iter()
                .map(|(name, value)| (name, TemplateValue::String(value))),
        );
        Self {
            config,
            parameters: SearchParameters::default(),
            categories: Vec::new(),
            limit: 100,
            offset: 0,
            skip_andmatch: false,
        }
    }

    pub(super) fn for_search(
        &self,
        keywords: &str,
        result: BTreeMap<String, String>,
    ) -> TemplateValue {
        self.for_search_with_parameters(keywords, result, &self.parameters, &self.categories)
    }

    pub(super) fn for_search_with_parameters(
        &self,
        keywords: &str,
        result: BTreeMap<String, String>,
        parameters: &SearchParameters,
        categories: &[String],
    ) -> TemplateValue {
        let mut query = HashMap::from([
            ("Album".to_owned(), TemplateValue::String(String::new())),
            ("Artist".to_owned(), TemplateValue::String(String::new())),
            ("Author".to_owned(), TemplateValue::String(String::new())),
            ("DoubanID".to_owned(), TemplateValue::String(String::new())),
            ("Season".to_owned(), TemplateValue::String(String::new())),
            ("Ep".to_owned(), TemplateValue::String(String::new())),
            ("Genre".to_owned(), TemplateValue::String(String::new())),
            ("IMDBID".to_owned(), TemplateValue::String(String::new())),
            (
                "IMDBIDShort".to_owned(),
                TemplateValue::String(String::new()),
            ),
            ("TMDBID".to_owned(), TemplateValue::String(String::new())),
            ("TVDBID".to_owned(), TemplateValue::String(String::new())),
            ("Title".to_owned(), TemplateValue::String(String::new())),
            ("Type".to_owned(), TemplateValue::String(String::new())),
            ("Year".to_owned(), TemplateValue::String(String::new())),
            ("id".to_owned(), TemplateValue::String(String::new())),
        ]);
        query.insert(
            "Keywords".to_owned(),
            TemplateValue::String(keywords.to_owned()),
        );
        for (field, value) in [
            ("Genre", parameters.genre.as_deref()),
            ("IMDBID", parameters.imdb_id.as_deref()),
            ("TMDBID", parameters.tmdb_id.as_deref()),
            ("Type", parameters.mode.as_deref()),
        ] {
            if let Some(value) = value {
                query.insert(field.to_owned(), TemplateValue::String(value.to_owned()));
            }
        }
        for (field, value) in [
            ("Year", parameters.year),
            ("Season", parameters.season),
            ("Ep", parameters.episode),
        ] {
            if let Some(value) = value {
                query.insert(field.to_owned(), TemplateValue::String(value.to_string()));
            }
        }
        let result = result
            .into_iter()
            .map(|(key, value)| (key, TemplateValue::String(value)))
            .collect();
        let mut render = TemplateValue::Object(HashMap::from([
            (
                "Config".to_owned(),
                TemplateValue::Object(self.config.clone()),
            ),
            (
                "Keywords".to_owned(),
                TemplateValue::String(keywords.to_owned()),
            ),
            (
                "keywords".to_owned(),
                TemplateValue::String(keywords.to_owned()),
            ),
            ("Query".to_owned(), TemplateValue::Object(query)),
            ("Result".to_owned(), TemplateValue::Object(result)),
            (
                "Categories".to_owned(),
                TemplateValue::Array(
                    categories
                        .iter()
                        .map(|value| TemplateValue::String(value.clone()))
                        .collect(),
                ),
            ),
            ("True".to_owned(), TemplateValue::Bool(true)),
            ("False".to_owned(), TemplateValue::Bool(false)),
            (
                "Today".to_owned(),
                TemplateValue::Object(HashMap::from([(
                    "Year".to_owned(),
                    TemplateValue::String(OffsetDateTime::now_utc().year().to_string()),
                )])),
            ),
        ]));
        set_pagination(&mut render, self.limit, self.offset);
        render
    }
}
