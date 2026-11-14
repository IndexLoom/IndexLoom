//! Cardigann result normalization; extraction lives in the sibling modules.
pub(super) use super::extract_html::parse_html_results;
pub(super) use super::extract_json::parse_json_results;
use super::{
    BTreeMap, EngineError, IndexerDefinition, JsonValue, Mapping, RenderContext, SearchResult, Url,
    Uuid, YamlValue, key, mapping, parse_date, parse_size, parse_u32, scalar,
};

pub(super) fn complete_result_fields(
    fields: &Mapping,
    result: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    fields
        .keys()
        .filter_map(scalar)
        .map(|name| {
            let name = name.split('|').next().unwrap_or_default().to_owned();
            let value = result.get(&name).cloned().unwrap_or_default();
            (name, value)
        })
        .collect()
}

/// Resolve helper-field dependencies without repeatedly applying stateful append modifiers.
#[allow(
    clippy::too_many_lines,
    reason = "Keep the dependency-resolution pass and field modifiers together."
)]
pub(super) fn fields(
    fields: &Mapping,
    context: &RenderContext,
    keywords: &str,
    mut extract: impl FnMut(&Mapping, &super::TemplateValue) -> Result<Option<String>, EngineError>,
) -> Result<Option<BTreeMap<String, String>>, EngineError> {
    if fields.len() > 1024 {
        return Err(EngineError::CardigannIncompatible(
            "more than 1024 result fields",
        ));
    }
    let references =
        super::Regex::new(r"\.Result\.([A-Za-z0-9_-]+)").expect("static field reference pattern");
    let mut pending = fields.iter().collect::<Vec<_>>();
    let mut values = BTreeMap::new();
    let mut done = std::collections::BTreeSet::new();
    let names = complete_result_fields(fields, &values);
    while !pending.is_empty() {
        let mut next = Vec::new();
        let before = pending.len();
        for (name_value, spec_value) in pending {
            let name = name_value;
            let spec = spec_value;
            let name = scalar(name).ok_or(EngineError::InvalidSearchDefinition(
                "field name must be scalar",
            ))?;
            let mut parts = name.split('|');
            let field = parts.next().unwrap_or_default();
            let modifiers: Vec<_> = parts.collect();
            if modifiers
                .iter()
                .any(|m| !["append", "optional", "default", "noappend"].contains(m))
            {
                return Err(EngineError::CardigannIncompatible(
                    "unsupported field modifier",
                ));
            }
            let spec = mapping(spec).ok_or(EngineError::InvalidSearchDefinition(
                "field must be a mapping",
            ))?;
            if spec.keys().filter_map(scalar).any(|key| {
                ![
                    "text",
                    "selector",
                    "attribute",
                    "optional",
                    "default",
                    "remove",
                    "case",
                    "filters",
                ]
                .contains(&key.as_str())
            }) {
                return Err(EngineError::CardigannIncompatible(
                    "unsupported field selector option",
                ));
            }
            let source = serde_yaml::to_string(spec)
                .map_err(|_| EngineError::InvalidSearchDefinition("invalid field"))?;
            if references
                .captures_iter(&source)
                .any(|r| names.contains_key(&r[1]) && !done.contains(&r[1]) && &r[1] != field)
            {
                next.push((name_value, spec_value));
                continue;
            }
            let render = context.for_search(keywords, complete_result_fields(fields, &values));
            let optional = [
                "imdb",
                "imdbid",
                "tmdbid",
                "rageid",
                "tvdbid",
                "tvmazeid",
                "traktid",
                "doubanid",
                "poster",
                "banner",
                "genre",
                "description",
            ]
            .contains(&field)
                || modifiers.contains(&"optional")
                || spec
                    .get(key("optional"))
                    .and_then(YamlValue::as_bool)
                    .unwrap_or(false);
            let value = extract(spec, &render)?;
            let value = match value {
                Some(v) if !v.is_empty() => Some(v),
                _ => spec
                    .get(key("default"))
                    .and_then(scalar)
                    .map(|v| super::render_template(&v, &render))
                    .transpose()?,
            };
            if let Some(value) = value {
                if modifiers.contains(&"append") {
                    values
                        .entry(field.to_owned())
                        .or_insert_with(String::new)
                        .push_str(&value);
                } else if !modifiers.contains(&"default") || !values.contains_key(field) {
                    values.insert(field.to_owned(), value);
                }
            } else if !optional {
                return Ok(None);
            }
            done.insert(field.to_owned());
        }
        if next.len() == before {
            return Err(EngineError::CardigannIncompatible(
                "cyclic result-field dependencies",
            ));
        }
        pending = next;
    }
    Ok(Some(values))
}

pub(super) fn finish(
    value: Option<String>,
    spec: &Mapping,
    render: &super::TemplateValue,
    encoding: &str,
) -> Result<Option<String>, EngineError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_owned();
    let value = if let Some(filters) = spec.get(key("filters")) {
        super::transform::apply(value, filters, render, encoding)?
    } else {
        value
    };
    Ok(Some(value))
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
        published_at: values.get("date").and_then(|value| {
            parse_date(value)
                .or_else(|| super::dates::fuzzy(value, None, super::OffsetDateTime::now_utc()))
        }),
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

pub(super) fn json_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(values) => values.iter().map(json_string).collect::<Vec<_>>().join(","),
        JsonValue::Object(_) => value.to_string(),
    }
}
