//! JSON rows, selector attributes, cases and field pipelines.
use super::{
    BTreeMap, EngineError, IndexerDefinition, JsonValue, Mapping, RenderContext, SearchResult, Url,
    key, mapping, render_template, scalar,
};
use super::{extract, jsonpath, rows};
pub(super) fn parse_json_results(
    instance: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let document: JsonValue = serde_json::from_str(body)?;
    let rows = rows::rows(search)?;
    let fields = rows::field_specs(search)?;
    let filters = rows::RowFilters::parse(rows)?;
    let render = context.for_search(keywords, BTreeMap::new());
    let encoding = &definition.spec.implementation.encoding;
    if let Some(count) = rows.get(key("count")).and_then(mapping)
        && field(&document, count, &render, encoding)?.is_some_and(|v| v.parse::<u64>() == Ok(0))
    {
        return Ok(Vec::new());
    }
    let path = rows
        .get(key("selector"))
        .and_then(scalar)
        .unwrap_or_default();
    let path = render_template(&path, &render)?;
    // Apply Cardigann predicates to individual array rows, after selecting the array.
    let split = predicate_start(&path);
    let mut selected = Vec::new();
    for node in jsonpath::select(&document, &path[..split])? {
        if let Some(array) = node.as_array() {
            selected.extend(array.iter());
        } else {
            selected.push(node);
        }
    }
    if selected.is_empty()
        && !rows::boolean(rows, "missingAttributeEqualsNoResults")
        && !path.is_empty()
    {
        // An existing empty array is a valid response; a missing selector is a definition/response error.
        if jsonpath::select(&document, &path[..split])?.is_empty() {
            return Err(EngineError::CardigannMissingField);
        }
    }
    let mut results = Vec::new();
    for selected in selected {
        if split < path.len() && jsonpath::select(selected, &path[split..])?.is_empty() {
            continue;
        }
        let attribute = rows.get(key("attribute")).and_then(scalar);
        let targets = if let Some(path) = attribute {
            jsonpath::select(selected, &render_template(&path, &render)?)?
        } else {
            vec![selected]
        };
        if targets.is_empty() && !rows::boolean(rows, "missingAttributeEqualsNoResults") {
            return Err(EngineError::CardigannMissingField);
        }
        for target in targets {
            let targets = if rows::boolean(rows, "multiple") {
                match target {
                    JsonValue::Array(v) => v.iter().collect::<Vec<_>>(),
                    JsonValue::Object(v) => v.values().collect(),
                    _ => {
                        return Err(EngineError::CardigannIncompatible(
                            "rows.multiple expects array or object",
                        ));
                    }
                }
            } else {
                vec![target]
            };
            for row in targets {
                let Some(values) = extract::fields(fields, context, keywords, |spec, render| {
                    field(row, spec, render, encoding)
                })?
                else {
                    continue;
                };
                if !context.skip_andmatch
                    && !filters.allows(values.get("title").map_or("", String::as_str), keywords)
                {
                    continue;
                }
                if let Some(result) =
                    extract::normalized_result(instance, definition, &values, final_url)
                {
                    results.push(result);
                }
            }
        }
    }
    Ok(results)
}
fn predicate_start(path: &str) -> usize {
    let mut quote = None;
    let mut escaped = false;
    let mut brackets = 0;
    for (i, c) in path.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == '[' {
            brackets += 1;
        } else if c == ']' {
            brackets -= 1;
        } else if c == ':' && brackets == 0 {
            return i;
        }
    }
    path.len()
}
pub(super) fn field(
    row: &JsonValue,
    spec: &Mapping,
    render: &super::TemplateValue,
    encoding: &str,
) -> Result<Option<String>, EngineError> {
    let mut value = if let Some(text) = spec.get(key("text")).and_then(scalar) {
        Some(render_template(&text, render)?)
    } else if let Some(path) = spec.get(key("selector")).and_then(scalar) {
        let selected = jsonpath::select(row, &render_template(&path, render)?)?;
        if selected.is_empty() {
            None
        } else {
            Some(
                selected
                    .iter()
                    .map(|v| extract::json_string(v))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    } else {
        None
    };
    if !spec.contains_key(key("text"))
        && let Some(cases) = spec.get(key("case")).and_then(mapping)
    {
        if let Some(value_ref) = value.as_ref() {
            value = Some(super::filters::apply_case(value_ref, cases));
        } else if let Some(fallback) = cases.get(key("*")).and_then(scalar) {
            value = Some(fallback);
        }
        value = value.map(|v| render_template(&v, render)).transpose()?;
    }
    extract::finish(value, spec, render, encoding)
}
