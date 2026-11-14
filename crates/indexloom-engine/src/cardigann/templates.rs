//! Cardigann templates and bounded declarative request planning.
use super::{
    Context, EngineError, FuncError, HYPHENATED_CONFIG_REFERENCE, HeaderName, IndexerDefinition,
    Mapping, SearchParameters, Template, TemplateValue, YamlValue, key, mapping, scalar,
};
use std::fmt::Write as _;
pub(super) fn render_template(
    source: &str,
    context: &TemplateValue,
) -> Result<String, EngineError> {
    if !source.contains("{{") {
        return Ok(source.to_owned());
    }
    let mut template = Template::default();
    template.add_func("join", template_join);
    template.add_func("wireencode", template_wire_encode);
    template.add_func("re_replace", template_regex_replace);
    let source = HYPHENATED_CONFIG_REFERENCE
        .replace_all(source, |capture: &regex::Captures<'_>| {
            format!(r#"(index .Config "{}")"#, &capture[1])
        });
    template
        .parse(source.as_ref())
        .map_err(|error| EngineError::Template(error.to_string()))?;
    template
        .render(&Context::from(context.clone()))
        .map_err(|error| EngineError::Template(error.to_string()))
}

pub(super) fn template_join(arguments: &[TemplateValue]) -> Result<TemplateValue, FuncError> {
    if arguments.len() != 2 {
        return Err(FuncError::ExactlyXArgs("join".to_owned(), 2));
    }
    let separator = arguments[1].to_string();
    let TemplateValue::Array(values) = &arguments[0] else {
        return Err(FuncError::Generic("join requires an array".to_owned()));
    };
    Ok(TemplateValue::String(
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(&separator),
    ))
}

pub(super) fn template_regex_replace(
    arguments: &[TemplateValue],
) -> Result<TemplateValue, FuncError> {
    if arguments.len() != 3 {
        return Err(FuncError::ExactlyXArgs("re_replace".to_owned(), 3));
    }
    let rendered = super::transform::regex_replace(
        &arguments[0].to_string(),
        &arguments[1].to_string(),
        &arguments[2].to_string(),
    )
    .map_err(|_| FuncError::Generic("invalid or over-budget regular expression".to_owned()))?;
    Ok(TemplateValue::String(rendered))
}

/// Translate Torznab/Newznab numeric categories to canonical tracker categories.
/// The 1000-wide blocks are standardized across compatible clients.
pub(super) fn standard_category(code: &str) -> Option<&'static str> {
    let code: u16 = code.parse().ok()?;
    match code / 1000 {
        1 => Some("Console"),
        2 => Some("Movies"),
        3 => Some("Audio"),
        4 => Some("PC"),
        5 => Some("TV"),
        6 => Some("XXX"),
        7 => Some("Books"),
        8 => Some("Other"),
        _ => None,
    }
}

pub(super) fn resolve_category_ids(
    definition: &IndexerDefinition,
    requested: Option<&str>,
) -> Vec<String> {
    let Some(requested) = requested else {
        return Vec::new();
    };
    let Some(caps) = mapping(&definition.spec.implementation.capabilities) else {
        return Vec::new();
    };
    let wanted = requested
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(32)
        .collect::<Vec<_>>();
    let matches = |normalized: &str, native: &str| {
        wanted.iter().any(|value| {
            *value == native
                || *value == normalized
                || normalized.starts_with(&format!("{value}/"))
                || standard_category(value).is_some_and(|root| {
                    normalized == root || normalized.starts_with(&format!("{root}/"))
                })
        })
    };
    let mut ids = std::collections::BTreeSet::new();
    if let Some(mappings) = caps
        .get(key("categorymappings"))
        .and_then(YamlValue::as_sequence)
    {
        for entry in mappings.iter().filter_map(mapping) {
            if let (Some(category), Some(native)) = (
                entry.get(key("cat")).and_then(scalar),
                entry.get(key("id")).and_then(scalar),
            ) && matches(&category, &native)
            {
                ids.insert(native);
            }
        }
    }
    if let Some(categories) = caps.get(key("categories")).and_then(mapping) {
        for (name, values) in categories {
            if let Some(category) = scalar(name) {
                for native in values
                    .as_sequence()
                    .into_iter()
                    .flatten()
                    .filter_map(scalar)
                {
                    if matches(&category, &native) {
                        ids.insert(native);
                    }
                }
            }
        }
    }
    ids.into_iter().collect()
}

pub(super) fn all_search_paths(search: &Mapping) -> Result<Vec<&Mapping>, EngineError> {
    if let Some(paths) = search.get(key("paths")).and_then(YamlValue::as_sequence) {
        if paths.is_empty() {
            return Err(EngineError::InvalidSearchDefinition(
                "search paths are empty",
            ));
        }
        return paths
            .iter()
            .map(|value| {
                mapping(value).ok_or(EngineError::InvalidSearchDefinition(
                    "search path must be a mapping",
                ))
            })
            .collect();
    }
    if search.contains_key(key("path")) {
        return Ok(vec![search]);
    }
    Err(EngineError::InvalidSearchDefinition(
        "search path is missing",
    ))
}

pub(super) fn retryable_search_error(error: &EngineError) -> bool {
    match error {
        EngineError::HttpClient(error) => error.is_timeout() || error.is_connect(),
        EngineError::UnexpectedStatus(status) => (500..=599).contains(status),
        _ => false,
    }
}

pub(super) fn render_inputs(
    value: Option<&YamlValue>,
    context: &TemplateValue,
    allow_empty: bool,
) -> Result<Vec<(String, String)>, EngineError> {
    let Some(inputs) = value.and_then(mapping) else {
        return Ok(Vec::new());
    };
    inputs
        .iter()
        .filter_map(|(name, value)| {
            let name = scalar(name)?;
            if name == "$raw" {
                return None;
            }
            Some((name, value))
        })
        .map(|(name, value)| {
            let rendered = render_template(&scalar(value).unwrap_or_default(), context)?;
            Ok((name, rendered))
        })
        .filter(|result: &Result<(String, String), EngineError>| {
            allow_empty
                || match result {
                    Ok((_, value)) => !value.is_empty(),
                    Err(_) => true,
                }
        })
        .collect()
}

pub(super) fn render_search_inputs(
    search: &Mapping,
    path: &Mapping,
    context: &TemplateValue,
    allow_empty: bool,
) -> Result<Vec<(String, String)>, EngineError> {
    let mut inputs = if path.get(key("inheritinputs")).and_then(YamlValue::as_bool) == Some(false) {
        Vec::new()
    } else {
        render_inputs(search.get(key("inputs")), context, allow_empty)?
    };
    // Path values override inherited values; don't send duplicate conflicting keys.
    for (name, value) in render_inputs(path.get(key("inputs")), context, allow_empty)? {
        inputs.retain(|(key, _)| key != &name);
        inputs.push((name, value));
    }
    Ok(inputs)
}

pub(super) fn render_headers(
    value: Option<&YamlValue>,
    context: &TemplateValue,
) -> Result<Vec<(HeaderName, String)>, EngineError> {
    let Some(headers) = value.and_then(mapping) else {
        return Ok(Vec::new());
    };
    headers
        .iter()
        .filter_map(|(name, values)| Some((scalar(name)?, values)))
        .map(|(name, values)| {
            let value = match values {
                YamlValue::Sequence(values) => values
                    .iter()
                    .filter_map(scalar)
                    .map(|value| render_template(&value, context))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", "),
                _ => render_template(&scalar(values).unwrap_or_default(), context)?,
            };
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| EngineError::InvalidHeaderName(name))?;
            Ok((name, value))
        })
        .collect()
}

pub(super) fn set_pagination(render: &mut TemplateValue, limit: usize, offset: usize) {
    if let TemplateValue::Object(root) = render
        && let Some(TemplateValue::Object(query)) = root.get_mut("Query")
    {
        query.insert("Limit".to_owned(), TemplateValue::from(limit.to_string()));
        query.insert("Offset".to_owned(), TemplateValue::from(offset.to_string()));
    }
}
pub(super) fn path_matches_categories(
    path: &Mapping,
    categories: &[String],
) -> Result<bool, EngineError> {
    let Some(restrictions) = path.get(key("categories")) else {
        return Ok(true);
    };
    let restrictions = restrictions
        .as_sequence()
        .ok_or(EngineError::InvalidSearchDefinition(
            "path categories must be a sequence",
        ))?;
    if categories.is_empty() {
        return Ok(true);
    }
    let restrictions = restrictions
        .iter()
        .map(|v| {
            scalar(v).ok_or(EngineError::InvalidSearchDefinition(
                "path category must be scalar",
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let exclude = restrictions.first().is_some_and(|v| v == "!");
    let intersects = restrictions.iter().any(|v| categories.contains(v));
    Ok(intersects != exclude)
}
pub(super) fn render_raw_query(
    search: &Mapping,
    path: &Mapping,
    render: &TemplateValue,
    encoding: &str,
) -> Result<Option<String>, EngineError> {
    let scopes = if path.get(key("inheritinputs")).and_then(YamlValue::as_bool) == Some(false) {
        vec![path]
    } else {
        vec![search, path]
    };
    let raw = scopes
        .into_iter()
        .filter_map(|scope| {
            scope
                .get(key("inputs"))
                .and_then(mapping)
                .and_then(|v| v.get(key("$raw")))
                .and_then(scalar)
        })
        .map(|v| render_encoded_template(&v, render, encoding, false))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!raw.is_empty()).then(|| raw.join("&")))
}

pub(super) fn supports_id_search(
    definition: &IndexerDefinition,
    parameters: &SearchParameters,
) -> bool {
    let Some(modes) = mapping(&definition.spec.implementation.capabilities)
        .and_then(|v| v.get(key("modes")))
        .and_then(mapping)
    else {
        return false;
    };
    [
        ("imdbid", parameters.imdb_id.as_ref()),
        ("tmdbid", parameters.tmdb_id.as_ref()),
    ]
    .iter()
    .any(|(field, id)| {
        id.is_some()
            && modes
                .values()
                .filter_map(YamlValue::as_sequence)
                .flatten()
                .filter_map(scalar)
                .any(|v| v.eq_ignore_ascii_case(field))
    })
}

/// Escape emitted template values, leaving static path/query punctuation and
/// control expressions intact. Encoding values after URL parsing loses legacy bytes.
pub(super) fn render_encoded_template(
    source: &str,
    context: &TemplateValue,
    encoding: &str,
    path: bool,
) -> Result<String, EngineError> {
    let mut rewritten = String::new();
    let mut rest = source;
    while let Some(open) = rest.find("{{") {
        rewritten.push_str(&rest[..open]);
        rest = &rest[open + 2..];
        let mut quote = None;
        let mut escaped = false;
        let mut close = None;
        let comment_end = if rest
            .trim_start()
            .trim_start_matches('-')
            .trim_start()
            .starts_with("/*")
        {
            rest.find("*/").map(|end| end + 2)
        } else {
            None
        };
        for (index, c) in rest.char_indices() {
            if comment_end.is_some_and(|end| index < end) {
                continue;
            }
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' && quote.is_some() {
                escaped = true;
                continue;
            }
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                continue;
            }
            if ['\'', '"', '`'].contains(&c) {
                quote = Some(c);
                continue;
            }
            if rest[index..].starts_with("}}") {
                close = Some(index);
                break;
            }
        }
        let close =
            close.ok_or_else(|| EngineError::Template("unclosed URL template action".into()))?;
        let action = rest[..close].trim();
        let left_trim = action.starts_with('-');
        let right_trim = action.ends_with('-');
        let action = action.trim_start_matches('-').trim_end_matches('-').trim();
        let head = action.split_whitespace().next().unwrap_or_default();
        let control = [
            "if", "else", "end", "range", "with", "define", "template", "block", "break",
            "continue",
        ]
        .contains(&head)
            || action.starts_with("/*")
            || action.contains(":=");
        if control {
            rewritten.push_str("{{");
            rewritten.push_str(&rest[..close]);
            rewritten.push_str("}}");
        } else {
            rewritten.push_str(if left_trim { "{{- " } else { "{{ " });
            write!(rewritten, "wireencode ({action}) {encoding:?} {path}")
                .expect("writing a String is infallible");
            rewritten.push_str(if right_trim { " -}}" } else { " }}" });
        }
        rest = &rest[close + 2..];
    }
    rewritten.push_str(rest);
    render_template(&rewritten, context)
}
fn template_wire_encode(args: &[TemplateValue]) -> Result<TemplateValue, FuncError> {
    if args.len() != 3 {
        return Err(FuncError::ExactlyXArgs("wireencode".into(), 3));
    }
    let wire = super::transport::WireEncoding::new(&args[1].to_string())
        .map_err(|_| FuncError::Generic("unsupported URL encoding".into()))?;
    let mut escaped = wire.escape(&args[0].to_string()).map_err(|_| {
        FuncError::Generic("URL value cannot be represented in the declared encoding".into())
    })?;
    if matches!(args[2], TemplateValue::Bool(true)) {
        escaped = escaped.replace('+', "%20");
    }
    Ok(TemplateValue::String(escaped))
}
