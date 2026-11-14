//! Shared preprocessing and response dispatch for HTML, XML and JSON.
use super::{
    BTreeMap, EngineError, Mapping, RenderContext, SearchResult, extract_html, key, mapping,
    parse_html_results, parse_json_results, scalar, transform, xml,
};
impl super::CardigannExecutor {
    pub(super) fn parse_search_response(
        &self,
        search: &Mapping,
        path: &Mapping,
        response: super::ExecutionResponse,
        context: &RenderContext,
        keywords: &str,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let render = context.for_search(keywords, BTreeMap::new());
        if let Some(message) = path
            .get(key("response"))
            .and_then(mapping)
            .and_then(|r| r.get(key("noResultsMessage")))
            .and_then(scalar)
            && ((message.is_empty() && response.body.trim().is_empty())
                || (!message.is_empty() && response.body.contains(&message)))
        {
            return Ok(Vec::new());
        }
        let mut body = response.body;
        if let Some(filters) = search.get(key("preprocessingfilters")) {
            body = transform::apply(
                body,
                filters,
                &render,
                &self.definition.spec.implementation.encoding,
            )?;
        }
        let parsed = match response.response_type.as_str() {
            "json" => parse_json_results(
                &self.indexer_instance_id,
                &self.definition,
                search,
                &body,
                context,
                keywords,
                &response.final_url,
            )?,
            "html" => parse_html_results(
                &self.indexer_instance_id,
                &self.definition,
                search,
                &body,
                context,
                keywords,
                &response.final_url,
            )?,
            "xml" => extract_html::parse_document_results(
                &self.indexer_instance_id,
                &self.definition,
                search,
                xml::document(&body)?,
                context,
                keywords,
                &response.final_url,
            )?,
            _ => return Err(EngineError::UnsupportedResponseType(response.response_type)),
        };
        Ok(parsed)
    }
}
