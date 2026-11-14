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

fn build_query_scope(
    keywords: &str,
    parameters: &SearchParameters,
) -> HashMap<String, TemplateValue> {
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
        ("TVMazeID".to_owned(), TemplateValue::String(String::new())),
        ("TraktID".to_owned(), TemplateValue::String(String::new())),
        ("TVRageID".to_owned(), TemplateValue::String(String::new())),
        ("Publisher".to_owned(), TemplateValue::String(String::new())),
        ("Label".to_owned(), TemplateValue::String(String::new())),
        ("Track".to_owned(), TemplateValue::String(String::new())),
        ("Group".to_owned(), TemplateValue::String(String::new())),
        ("MaxAge".to_owned(), TemplateValue::String(String::new())),
        ("MinSize".to_owned(), TemplateValue::String(String::new())),
        ("MaxSize".to_owned(), TemplateValue::String(String::new())),
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
        ("TVDBID", parameters.tvdb_id.as_deref()),
        ("TVMazeID", parameters.tvmaze_id.as_deref()),
        ("TraktID", parameters.trakt_id.as_deref()),
        ("TVRageID", parameters.tvrage_id.as_deref()),
        ("DoubanID", parameters.douban_id.as_deref()),
        ("Artist", parameters.artist.as_deref()),
        ("Album", parameters.album.as_deref()),
        ("Author", parameters.author.as_deref()),
        ("Title", parameters.title.as_deref()),
        ("Publisher", parameters.publisher.as_deref()),
        ("Label", parameters.label.as_deref()),
        ("Track", parameters.track.as_deref()),
        ("Group", parameters.group.as_deref()),
    ] {
        if let Some(value) = value {
            query.insert(field.to_owned(), TemplateValue::String(value.to_owned()));
        }
    }
    for (field, value) in [
        ("Year", parameters.year),
        ("Season", parameters.season),
        ("Ep", parameters.episode),
        (
            "MaxAge",
            parameters.max_age_days.and_then(|v| u16::try_from(v).ok()),
        ),
    ] {
        if let Some(value) = value {
            query.insert(field.to_owned(), TemplateValue::String(value.to_string()));
        }
    }
    if let Some(episode) = parameters.episode_code.as_deref() {
        query.insert("Ep".to_owned(), TemplateValue::String(episode.to_owned()));
    }
    for (field, value) in [
        ("MinSize", parameters.min_size),
        ("MaxSize", parameters.max_size),
    ] {
        if let Some(value) = value {
            query.insert(field.to_owned(), TemplateValue::String(value.to_string()));
        }
    }
    query
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

    #[allow(
        clippy::too_many_lines,
        reason = "Declarative Cardigann query values are assembled together for template compatibility."
    )]
    pub(super) fn for_search_with_parameters(
        &self,
        keywords: &str,
        result: BTreeMap<String, String>,
        parameters: &SearchParameters,
        categories: &[String],
    ) -> TemplateValue {
        let query = build_query_scope(keywords, parameters);
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
