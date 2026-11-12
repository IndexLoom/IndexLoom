//! Cardigann filters, selectors, primitive parsers and dates.
use super::{
    BrowserCookie, CARDIGANN_REGEX_BACKTRACK_LIMIT, ConfigurationValue, ElementRef, EngineError,
    FancyRegex, FancyRegexBuilder, Html, JsonValue, Mapping, OffsetDateTime, Regex, Selector,
    TemplateValue, Url, YamlValue,
};

pub(super) fn apply_case(value: &str, cases: &Mapping) -> String {
    let mut fallback = None;
    for (candidate, replacement) in cases {
        let Some(candidate) = scalar(candidate) else {
            continue;
        };
        let replacement = scalar(replacement).unwrap_or_default();
        if candidate == "*" {
            fallback = Some(replacement);
        } else if candidate.eq_ignore_ascii_case(value) {
            return replacement;
        }
    }
    fallback.unwrap_or_else(|| value.to_owned())
}

pub(super) fn apply_filters(mut value: String, filters: &YamlValue) -> Result<String, EngineError> {
    let Some(filters) = filters.as_sequence() else {
        return Ok(value);
    };
    for filter in filters {
        let Some(filter) = mapping(filter) else {
            continue;
        };
        let name = filter.get(key("name")).and_then(scalar).unwrap_or_default();
        let args = filter_args(filter.get(key("args")));
        match name.as_str() {
            "replace" if args.len() >= 2 => value = value.replace(&args[0], &args[1]),
            "re_replace" if args.len() >= 2 => {
                let regex = cardigann_regex(&args[0])?;
                value = regex
                    .try_replacen(&value, 0, args[1].as_str())
                    .map_err(|error| EngineError::InvalidFilter(error.to_string()))?
                    .into_owned();
            }
            "regexp" if !args.is_empty() => {
                let regex = cardigann_regex(&args[0])?;
                value = regex
                    .captures(&value)
                    .map_err(|error| EngineError::InvalidFilter(error.to_string()))?
                    .and_then(|captures| captures.get(1).or_else(|| captures.get(0)))
                    .map_or_else(String::new, |capture| capture.as_str().to_owned());
            }
            "append" if !args.is_empty() => value.push_str(&args.join("")),
            "prepend" if !args.is_empty() => value = format!("{}{value}", args.join("")),
            "trim" => {
                let trimmed = value.trim().to_owned();
                value = trimmed;
            }
            "split" if args.len() >= 2 => {
                let index = args[1].parse::<usize>().unwrap_or(0);
                let part = value
                    .split(&args[0])
                    .nth(index)
                    .unwrap_or_default()
                    .to_owned();
                value = part;
            }
            "tolower" => value = value.to_lowercase(),
            "toupper" => value = value.to_uppercase(),
            "urlencode" => value = url::form_urlencoded::byte_serialize(value.as_bytes()).collect(),
            "urldecode" => {
                let encoded = format!("value={value}");
                value = url::form_urlencoded::parse(encoded.as_bytes())
                    .next()
                    .map_or_else(String::new, |(_, decoded)| decoded.into_owned());
            }
            "querystring" if !args.is_empty() => {
                if let Ok(url) = Url::parse(&value) {
                    value = url
                        .query_pairs()
                        .find(|(name, _)| name.as_ref() == args[0])
                        .map_or_else(String::new, |(_, value)| value.into_owned());
                }
            }
            // dateparse/timeago/fuzzytime are normalized later when possible;
            // validation and display-only filters do not alter the source text.
            _ => {}
        }
    }
    Ok(value)
}

pub(super) fn cardigann_regex(pattern: &str) -> Result<FancyRegex, EngineError> {
    let mut builder = FancyRegexBuilder::new(pattern);
    builder.backtrack_limit(CARDIGANN_REGEX_BACKTRACK_LIMIT);
    builder
        .build()
        .map_err(|error| EngineError::InvalidFilter(error.to_string()))
}

pub(super) fn filter_args(value: Option<&YamlValue>) -> Vec<String> {
    match value {
        Some(YamlValue::Sequence(values)) => values.iter().filter_map(scalar).collect(),
        Some(value) => scalar(value).into_iter().collect(),
        None => Vec::new(),
    }
}

pub(super) fn normalize_contains_selector(value: &str) -> (String, Vec<String>) {
    let regex = Regex::new(r#":contains\((?:"([^"]*)"|'([^']*)'|([^)]*))\)"#)
        .expect("static contains regex must compile");
    let contains = regex
        .captures_iter(value)
        .filter_map(|capture| {
            capture
                .get(1)
                .or_else(|| capture.get(2))
                .or_else(|| capture.get(3))
        })
        .map(|capture| capture.as_str().trim().to_owned())
        .collect();
    (regex.replace_all(value, "").into_owned(), contains)
}

pub(super) fn check_error_selectors(
    value: Option<&YamlValue>,
    body: &str,
) -> Result<(), EngineError> {
    let Some(errors) = value.and_then(YamlValue::as_sequence) else {
        return Ok(());
    };
    for error in errors {
        let Some(error) = mapping(error) else {
            continue;
        };
        if let Some(selector) = error.get(key("selector")).and_then(scalar)
            && selector_matches(body, &selector)?
        {
            return Err(EngineError::AuthenticationRejected);
        }
    }
    Ok(())
}

pub(super) fn selector_matches(body: &str, selector: &str) -> Result<bool, EngineError> {
    let (css, contains) = normalize_contains_selector(selector);
    let selector =
        Selector::parse(&css).map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
    let document = Html::parse_document(body);
    Ok(document.select(&selector).any(|element| {
        contains
            .iter()
            .all(|needle| element_text(element).contains(needle))
    }))
}

pub(super) fn element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

pub(super) fn browser_cookies(header: Option<&str>) -> Vec<BrowserCookie> {
    header
        .into_iter()
        .flat_map(|header| header.split(';'))
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            Some(BrowserCookie {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
                domain: None,
                path: None,
                secure: false,
                http_only: false,
            })
        })
        .collect()
}

pub(super) fn mapping(value: &YamlValue) -> Option<&Mapping> {
    value.as_mapping()
}
pub(super) fn key(value: &str) -> YamlValue {
    YamlValue::String(value.to_owned())
}

pub(super) fn scalar(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Bool(value) => Some(value.to_string()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Null => Some(String::new()),
        _ => None,
    }
}

pub(super) fn configuration_json(value: ConfigurationValue) -> JsonValue {
    match value {
        ConfigurationValue::String(value) => JsonValue::String(value),
        ConfigurationValue::Boolean(value) => JsonValue::Bool(value),
        ConfigurationValue::Integer(value) => JsonValue::from(value),
        ConfigurationValue::Number(value) => JsonValue::from(value),
    }
}

pub(super) fn json_template_value(value: JsonValue) -> TemplateValue {
    match value {
        JsonValue::Null => TemplateValue::Nil,
        JsonValue::Bool(value) => TemplateValue::Bool(value),
        JsonValue::String(value) => TemplateValue::String(value),
        JsonValue::Number(value) => value
            .as_i64()
            .map(TemplateValue::from)
            .or_else(|| value.as_u64().map(TemplateValue::from))
            .or_else(|| value.as_f64().map(TemplateValue::from))
            .unwrap_or(TemplateValue::Nil),
        JsonValue::Array(values) => {
            TemplateValue::Array(values.into_iter().map(json_template_value).collect())
        }
        JsonValue::Object(values) => TemplateValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, json_template_value(value)))
                .collect(),
        ),
    }
}

pub(super) fn parse_u32(value: &str) -> Option<u32> {
    value.replace([',', ' '], "").parse().ok()
}

pub(super) fn parse_size(value: &str) -> Option<u64> {
    if let Ok(bytes) = value.replace([',', ' '], "").parse() {
        return Some(bytes);
    }
    let regex = Regex::new(r"(?i)([0-9]+(?:[.,][0-9]+)?)\s*([kmgtp]?i?b)").ok()?;
    let captures = regex.captures(value)?;
    let number = captures
        .get(1)?
        .as_str()
        .replace(',', ".")
        .parse::<f64>()
        .ok()?;
    let multiplier = match captures.get(2)?.as_str().to_ascii_lowercase().as_str() {
        "kb" | "kib" => 1024_f64,
        "mb" | "mib" => 1024_f64.powi(2),
        "gb" | "gib" => 1024_f64.powi(3),
        "tb" | "tib" => 1024_f64.powi(4),
        "pb" | "pib" => 1024_f64.powi(5),
        _ => 1_f64,
    };
    let bytes = number * multiplier;
    if !bytes.is_finite() || bytes.is_sign_negative() || bytes >= 18_446_744_073_709_551_616.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(bytes as u64)
}

pub(super) fn parse_date(value: &str) -> Option<OffsetDateTime> {
    use time::format_description::{
        parse_borrowed,
        well_known::{Rfc2822, Rfc3339},
    };
    let value = value.trim();
    if let Ok(timestamp) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(timestamp);
    }
    if let Ok(timestamp) = OffsetDateTime::parse(value, &Rfc2822) {
        return Some(timestamp);
    }
    // These timezone-free tracker formats are interpreted as UTC consistently.
    // Prefer explicit timezone formats above for sources that advertise them.
    for pattern in [
        "[year]-[month]-[day] [hour]:[minute]:[second]",
        "[year]-[month]-[day] [hour]:[minute]",
        "[year]/[month]/[day] [hour]:[minute]:[second]",
    ] {
        if let Ok(format) = parse_borrowed::<3>(pattern)
            && let Ok(timestamp) = time::PrimitiveDateTime::parse(value, &format)
        {
            return Some(timestamp.assume_utc());
        }
    }
    for pattern in ["[year]-[month]-[day]", "[year]/[month]/[day]"] {
        if let Ok(format) = parse_borrowed::<3>(pattern)
            && let Ok(date) = time::Date::parse(value, &format)
        {
            return Some(date.midnight().assume_utc());
        }
    }
    None
}
