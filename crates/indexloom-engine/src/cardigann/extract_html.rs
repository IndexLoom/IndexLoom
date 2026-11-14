//! HTML and XML field extraction with selector cases and non-mutating removals.
use super::{
    BTreeMap, ElementRef, EngineError, Html, IndexerDefinition, Mapping, RenderContext,
    SearchResult, Url, key, mapping, render_template, scalar,
};
use super::{extract, rows, selectors::Css};
pub(super) fn parse_html_results(
    instance: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    parse_document_results(
        instance,
        definition,
        search,
        Html::parse_document(body),
        context,
        keywords,
        final_url,
    )
}
pub(super) fn parse_document_results(
    instance: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    mut document: Html,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let rows = rows::rows(search)?;
    let fields = rows::field_specs(search)?;
    let filters = rows::RowFilters::parse(rows)?;
    let render = context.for_search(keywords, BTreeMap::new());
    let selector =
        rows.get(key("selector"))
            .and_then(scalar)
            .ok_or(EngineError::InvalidSearchDefinition(
                "row selector is missing",
            ))?;
    let selector = Css::parse(&render_template(&selector, &render)?)?;
    // Merge adjacent selected rows into a single DOM row before extracting fields.
    let after = rows
        .get(key("after"))
        .and_then(serde_yaml::Value::as_u64)
        .unwrap_or(0);
    if after > 100 {
        return Err(EngineError::CardigannIncompatible("rows.after exceeds 100"));
    }
    if after > 0 {
        let selected = selector
            .select(document.root_element(), true)?
            .iter()
            .map(|e| e.id())
            .collect::<Vec<_>>();
        let group = usize::try_from(after + 1).unwrap_or(101);
        if selected.len() % group != 0 {
            return Err(EngineError::CardigannIncompatible(
                "incomplete rows.after group",
            ));
        }
        for chunk in selected.chunks(group) {
            for other in &chunk[1..] {
                let children = document
                    .tree
                    .get(*other)
                    .unwrap()
                    .children()
                    .map(|e| e.id())
                    .collect::<Vec<_>>();
                for child in children {
                    document.tree.get_mut(chunk[0]).unwrap().append_id(child);
                }
                document.tree.get_mut(*other).unwrap().detach();
            }
        }
    }
    let mut results = Vec::new();
    for row in selector.select(document.root_element(), true)? {
        let Some(mut values) = extract::fields(fields, context, keywords, |spec, render| {
            field(row, spec, render, &definition.spec.implementation.encoding)
        })?
        else {
            continue;
        };
        if !context.skip_andmatch
            && !filters.allows(values.get("title").map_or("", String::as_str), keywords)
        {
            continue;
        }
        if !values.contains_key("date")
            && let Some(header) = rows.get(key("dateheaders")).and_then(mapping)
        {
            let mut found = None;
            let mut current = Some(row);
            while let Some(node) = current {
                if let Some(previous) = node.prev_siblings().find_map(ElementRef::wrap) {
                    if let Some(value) = field(
                        previous,
                        header,
                        &render,
                        &definition.spec.implementation.encoding,
                    )? {
                        found = Some(value);
                        break;
                    }
                    current = Some(previous);
                } else {
                    current = node.parent().and_then(ElementRef::wrap);
                }
            }
            if let Some(value) = found {
                values.insert("date".into(), value);
            } else if !rows::boolean(header, "optional") {
                continue;
            }
        }
        if let Some(result) = extract::normalized_result(instance, definition, &values, final_url) {
            results.push(result);
        }
    }
    Ok(results)
}
pub(super) fn field(
    row: ElementRef<'_>,
    spec: &Mapping,
    render: &super::TemplateValue,
    encoding: &str,
) -> Result<Option<String>, EngineError> {
    let value = if let Some(text) = spec.get(key("text")).and_then(scalar) {
        Some(render_template(&text, render)?)
    } else {
        let element = if let Some(selector) = spec.get(key("selector")).and_then(scalar) {
            let source = render_template(&selector, render)?;
            let selector = Css::parse(&source)?;
            let root = if source.starts_with(":root") {
                row.ancestors()
                    .filter_map(ElementRef::wrap)
                    .last()
                    .unwrap_or(row)
            } else {
                row
            };
            selector.select(root, true)?.first().copied()
        } else {
            Some(row)
        };
        let Some(element) = element else {
            return Ok(None);
        };
        let cleaned = spec
            .get(key("remove"))
            .and_then(scalar)
            .map(|selector| {
                let selector = render_template(&selector, render)?;
                super::selectors::without(element, &selector)
            })
            .transpose()?;
        let element = cleaned.as_ref().map_or(element, Html::root_element);

        if let Some(cases) = spec.get(key("case")).and_then(mapping) {
            let mut value = None;
            for (condition, replacement) in cases {
                let css = Css::parse(&render_template(condition, render)?)?;
                if !css.select(element, true)?.is_empty() {
                    value = Some(render_template(
                        &scalar(replacement).unwrap_or_default(),
                        render,
                    )?);
                    break;
                }
            }
            value
        } else if let Some(attribute) = spec.get(key("attribute")).and_then(scalar) {
            element.attr(&attribute).map(str::to_owned)
        } else {
            Some(super::selectors::text(element, None)?)
        }
    };
    extract::finish(value, spec, render, encoding)
}
