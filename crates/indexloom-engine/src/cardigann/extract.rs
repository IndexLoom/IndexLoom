//! Cardigann HTML/JSON extraction and normalized result fields.
use super::{
    BTreeMap, ElementRef, EngineError, Html, IndexerDefinition, JsonValue, Mapping, RenderContext,
    SearchResult, Selector, Url, Uuid, YamlValue, apply_case, apply_filters, element_text, key,
    mapping, normalize_contains_selector, parse_date, parse_size, parse_u32, render_template,
    scalar,
};

pub(super) fn parse_html_results(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let rows =
        search
            .get(key("rows"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search rows are missing",
            ))?;
    let selector =
        rows.get(key("selector"))
            .and_then(scalar)
            .ok_or(EngineError::InvalidSearchDefinition(
                "row selector is missing",
            ))?;
    let selector = render_template(&selector, &context.for_search(keywords, BTreeMap::new()))?;
    let (css, contains) = normalize_contains_selector(&selector);
    let selector =
        Selector::parse(&css).map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
    let document = Html::parse_document(body);
    document
        .select(&selector)
        .filter(|row| {
            contains
                .iter()
                .all(|needle| element_text(*row).contains(needle))
        })
        .filter_map(|row| {
            match extract_html_row(
                indexer_instance_id,
                definition,
                search,
                row,
                context,
                keywords,
                final_url,
            ) {
                Ok(Some(result)) => Some(Ok(result)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

pub(super) fn extract_html_row(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    row: ElementRef<'_>,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Option<SearchResult>, EngineError> {
    let fields =
        search
            .get(key("fields"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search fields are missing",
            ))?;
    let mut result = BTreeMap::new();
    for (name, specification) in fields {
        let Some(name) = scalar(name) else { continue };
        let Some(specification) = mapping(specification) else {
            continue;
        };
        if specification.contains_key(key("text")) {
            continue;
        }
        let render_result = complete_result_fields(fields, &result);
        if let Some(value) =
            extract_html_field(row, specification, context, keywords, &render_result)?
        {
            result.insert(name, value);
        }
    }
    for _ in 0..3 {
        let before = result.len();
        for (name, specification) in fields {
            let Some(name) = scalar(name) else { continue };
            if result.contains_key(&name) {
                continue;
            }
            let Some(specification) = mapping(specification) else {
                continue;
            };
            let render_result = complete_result_fields(fields, &result);
            if let Some(value) =
                extract_html_field(row, specification, context, keywords, &render_result)?
            {
                result.insert(name, value);
            }
        }
        if result.len() == before {
            break;
        }
    }
    Ok(normalized_result(
        indexer_instance_id,
        definition,
        &result,
        final_url,
    ))
}

pub(super) fn complete_result_fields(
    fields: &Mapping,
    result: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    fields
        .keys()
        .filter_map(scalar)
        .map(|name| {
            let value = result.get(&name).cloned().unwrap_or_default();
            (name, value)
        })
        .collect()
}

pub(super) fn extract_html_field(
    row: ElementRef<'_>,
    specification: &Mapping,
    context: &RenderContext,
    keywords: &str,
    result: &BTreeMap<String, String>,
) -> Result<Option<String>, EngineError> {
    let render = context.for_search(keywords, result.clone());
    let mut value = if let Some(text) = specification.get(key("text")).and_then(scalar) {
        render_template(&text, &render)?
    } else if let Some(selector_text) = specification.get(key("selector")).and_then(scalar) {
        let selector_text = render_template(&selector_text, &render)?;
        let (css, contains) = normalize_contains_selector(&selector_text);
        let selector = Selector::parse(&css)
            .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
        let element = row.select(&selector).find(|element| {
            contains
                .iter()
                .all(|needle| element_text(*element).contains(needle))
        });
        let Some(element) = element else {
            return Ok(specification.get(key("default")).and_then(scalar));
        };
        if let Some(attribute) = specification.get(key("attribute")).and_then(scalar) {
            element
                .value()
                .attr(&attribute)
                .unwrap_or_default()
                .to_owned()
        } else {
            element_text(element)
        }
    } else {
        specification
            .get(key("default"))
            .and_then(scalar)
            .unwrap_or_default()
    };
    if let Some(cases) = specification.get(key("case")).and_then(mapping) {
        value = apply_case(&value, cases);
    }
    if let Some(filters) = specification.get(key("filters")) {
        value = apply_filters(value, filters)?;
    }
    if value.trim().is_empty()
        && specification
            .get(key("optional"))
            .and_then(YamlValue::as_bool)
            .unwrap_or(false)
    {
        Ok(None)
    } else {
        Ok(Some(value.trim().to_owned()))
    }
}

pub(super) fn parse_json_results(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let document: JsonValue = serde_json::from_str(body)?;
    let rows =
        search
            .get(key("rows"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search rows are missing",
            ))?;
    let selector = rows
        .get(key("selector"))
        .and_then(scalar)
        .unwrap_or_default();
    let selected = json_path(&document, &selector).unwrap_or(&document);
    let rows = selected
        .as_array()
        .map_or_else(|| std::slice::from_ref(selected), Vec::as_slice);
    let attribute = rows_mapping_attribute(search);
    rows.iter()
        .filter_map(|row| {
            let row = attribute
                .as_deref()
                .and_then(|path| json_path(row, path))
                .unwrap_or(row);
            match extract_json_row(
                indexer_instance_id,
                definition,
                search,
                row,
                context,
                keywords,
                final_url,
            ) {
                Ok(Some(result)) => Some(Ok(result)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

pub(super) fn rows_mapping_attribute(search: &Mapping) -> Option<String> {
    search
        .get(key("rows"))
        .and_then(mapping)
        .and_then(|rows| rows.get(key("attribute")))
        .and_then(scalar)
}

pub(super) fn extract_json_row(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    row: &JsonValue,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Option<SearchResult>, EngineError> {
    let fields =
        search
            .get(key("fields"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search fields are missing",
            ))?;
    let mut result = BTreeMap::new();
    for _ in 0..3 {
        let before = result.len();
        for (name, specification) in fields {
            let Some(name) = scalar(name) else { continue };
            if result.contains_key(&name) {
                continue;
            }
            let Some(specification) = mapping(specification) else {
                continue;
            };
            let render = context.for_search(keywords, complete_result_fields(fields, &result));
            let mut value = if let Some(text) = specification.get(key("text")).and_then(scalar) {
                render_template(&text, &render)?
            } else if let Some(selector) = specification.get(key("selector")).and_then(scalar) {
                json_path(row, &selector)
                    .map(json_string)
                    .unwrap_or_default()
            } else {
                specification
                    .get(key("default"))
                    .and_then(scalar)
                    .unwrap_or_default()
            };
            if let Some(cases) = specification.get(key("case")).and_then(mapping) {
                value = apply_case(&value, cases);
            }
            if let Some(filters) = specification.get(key("filters")) {
                value = apply_filters(value, filters)?;
            }
            if !value.trim().is_empty() {
                result.insert(name, value.trim().to_owned());
            }
        }
        if result.len() == before {
            break;
        }
    }
    Ok(normalized_result(
        indexer_instance_id,
        definition,
        &result,
        final_url,
    ))
}

pub(super) fn normalized_result(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    values: &BTreeMap<String, String>,
    base: &Url,
) -> Option<SearchResult> {
    let title = values
        .get("title")
        .filter(|value| !value.is_empty())
        .cloned()?;
    let details_url =
        result_value(values, &["details"]).and_then(|value| normalize_http_link(base, value));
    let explicit_magnet = result_value(values, &["magnet"]).and_then(normalize_magnet);
    let raw_download = result_value(values, &["download"]);
    let magnet_url = explicit_magnet.or_else(|| raw_download.and_then(normalize_magnet));
    let download_url = raw_download.and_then(|value| normalize_http_link(base, value));
    if details_url.is_none() && download_url.is_none() && magnet_url.is_none() {
        return None;
    }
    let info_hash = result_value(values, &["infohash", "info_hash"])
        .and_then(normalize_info_hash)
        .or_else(|| magnet_url.as_deref().and_then(info_hash_from_magnet));
    Some(SearchResult {
        id: Uuid::now_v7(),
        indexer_instance_id: indexer_instance_id.to_owned(),
        definition_id: definition.metadata.id.clone(),
        source_id: result_value(
            values,
            &[
                "sourceid",
                "source_id",
                "torrentid",
                "torrent_id",
                "id",
                "_id",
            ],
        )
        .map(ToOwned::to_owned),
        source_guid: result_value(values, &["sourceguid", "source_guid", "guid"])
            .map(ToOwned::to_owned),
        info_hash,
        title,
        details_url,
        download_url,
        magnet_url,
        category: values.get("category").cloned(),
        size: values.get("size").and_then(|value| parse_size(value)),
        seeders: values.get("seeders").and_then(|value| parse_u32(value)),
        leechers: values.get("leechers").and_then(|value| parse_u32(value)),
        published_at: values.get("date").and_then(|value| parse_date(value)),
    })
}

pub(super) fn result_value<'a>(
    values: &'a BTreeMap<String, String>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| values.get(*key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn normalize_magnet(value: &str) -> Option<String> {
    let value = value.trim();
    let url = Url::parse(value).ok()?;
    if !url.scheme().eq_ignore_ascii_case("magnet") {
        return None;
    }
    let has_exact_topic = url.query_pairs().any(|(name, value)| {
        name.eq_ignore_ascii_case("xt")
            && (value
                .get(..9)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
                || value
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btmh:")))
    });
    has_exact_topic.then(|| value.to_owned())
}

pub(super) fn info_hash_from_magnet(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    url.query_pairs()
        .find(|(name, value)| {
            name.eq_ignore_ascii_case("xt")
                && value
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
        })
        .and_then(|(_, value)| normalize_info_hash(&value[9..]))
}

pub(super) fn normalize_info_hash(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .get(..9)
        .filter(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
        .map_or(value, |_| &value[9..]);
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(value.to_ascii_lowercase());
    }
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'2'..=b'7'))
    {
        return Some(value.to_ascii_uppercase());
    }
    None
}

pub(super) fn normalize_http_link(base: &Url, value: &str) -> Option<String> {
    let url = base.join(value).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

pub(super) fn json_path<'a>(mut value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if path.is_empty() || path == ":root" {
        return Some(value);
    }
    for segment in path.trim_start_matches("$.").split('.') {
        if segment.is_empty() {
            continue;
        }
        let mut name = segment;
        let mut index = None;
        if let Some(open) = segment.find('[') {
            name = &segment[..open];
            index = segment[open + 1..]
                .trim_end_matches(']')
                .parse::<usize>()
                .ok();
        }
        if !name.is_empty() {
            value = value.get(name)?;
        }
        if let Some(index) = index {
            value = value.get(index)?;
        }
    }
    Some(value)
}

pub(super) fn json_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(values) => values.iter().map(json_string).collect::<Vec<_>>().join(" "),
        JsonValue::Object(_) => value.to_string(),
    }
}
