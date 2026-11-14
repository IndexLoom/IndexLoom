//! Offline response evaluation for reproducible compatibility fixtures. Performs no HTTP.
use super::{
    BTreeMap, EngineError, IndexerDefinition, RenderContext, SearchResult, Url, extract_html, key,
    mapping, parse_html_results, parse_json_results, transform, xml,
};
/// Evaluate one recorded response using the production extraction/filter pipeline.
///
/// # Errors
/// Returns the same structured errors as live response parsing.
pub fn parse_response(
    definition: &IndexerDefinition,
    body: &str,
    format: &str,
) -> Result<Vec<SearchResult>, EngineError> {
    let search = mapping(&definition.spec.implementation.search).ok_or(
        EngineError::CardigannIncompatible("search must be a mapping"),
    )?;
    let context = RenderContext::new(BTreeMap::new(), BTreeMap::new());
    let render = context.for_search("Example Movie", BTreeMap::new());
    let body = search.get(key("preprocessingfilters")).map_or_else(
        || Ok(body.into()),
        |filters| transform::apply(body.into(), filters, &render, "UTF-8"),
    )?;
    let url = Url::parse("https://example.invalid/")?;
    match format {
        "json" => parse_json_results(
            "fixture",
            definition,
            search,
            &body,
            &context,
            "Example Movie",
            &url,
        ),
        "xml" => extract_html::parse_document_results(
            "fixture",
            definition,
            search,
            xml::document(&body)?,
            &context,
            "Example Movie",
            &url,
        ),
        "html" => parse_html_results(
            "fixture",
            definition,
            search,
            &body,
            &context,
            "Example Movie",
            &url,
        ),
        _ => Err(EngineError::CardigannIncompatible(
            "unknown fixture response format",
        )),
    }
}
