//! Cardigann filters, selectors, primitive parsers and dates.
use super::{
    CARDIGANN_REGEX_BACKTRACK_LIMIT, ConfigurationValue, EngineError, FancyRegex,
    FancyRegexBuilder, Html, JsonValue, Mapping, OffsetDateTime, Regex, TemplateValue, YamlValue,
};

pub(super) fn apply_case(value: &str, cases: &Mapping) -> String {
    let mut fallback = None;
    for (candidate, replacement) in cases {
        let candidate = candidate.clone();
        let replacement = scalar(replacement).unwrap_or_default();
        if candidate == "*" {
            fallback = Some(replacement);
        } else if candidate.eq_ignore_ascii_case(value) {
            return replacement;
        }
    }
    fallback.unwrap_or_else(|| value.to_owned())
}

pub(super) fn apply_filters(value: String, filters: &YamlValue) -> Result<String, EngineError> {
    super::transform::apply(value, filters, &TemplateValue::Nil, "UTF-8")
}

pub(super) fn cardigann_regex(pattern: &str) -> Result<FancyRegex, EngineError> {
    if pattern.len() > 16_384 {
        return Err(EngineError::CardigannFilter {
            name: "regex".into(),
            reason: "pattern exceeds 16 KiB limit",
        });
    }
    let mut builder = FancyRegexBuilder::new(pattern);
    builder.backtrack_limit(CARDIGANN_REGEX_BACKTRACK_LIMIT);
    builder.delegate_size_limit(2 * 1024 * 1024);
    builder.delegate_dfa_size_limit(4 * 1024 * 1024);
    builder.build().map_err(|_| EngineError::CardigannFilter {
        name: "regex".into(),
        reason: "invalid regular expression",
    })
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
    let Some(errors) = value.and_then(YamlValue::as_array) else {
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
    let selector = super::selectors::Css::parse(selector)?;
    let document = Html::parse_document(body);
    Ok(!selector.select(document.root_element(), true)?.is_empty())
}

pub(super) fn mapping(value: &YamlValue) -> Option<&Mapping> {
    value.as_object()
}
pub(super) fn key(value: &str) -> &str {
    value
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
