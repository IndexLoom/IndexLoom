//! Original implementation of Cardigann download workflows. Only the server
//! should turn user release IDs into the execution-only `SearchResult` input.
use super::http::BinaryResponse;
use super::{
    BTreeMap, CardigannExecutor, EngineError, HeaderName, Html, Mapping, Method, RequestSpec,
    SearchResult, Selector, TemplateValue, Url, YamlValue, apply_filters, element_text, key,
    mapping, normalize_contains_selector, render_headers, render_inputs, render_template, scalar,
};
use std::collections::HashMap;

mod torrent;
pub use torrent::validate_torrent;

/// Validated download material. No request headers or session cookies are exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadArtifact {
    Torrent(Vec<u8>),
    Magnet(String),
    Nzb(Vec<u8>),
}

/// Validates a `BitTorrent` v1/v2 exact topic rather than merely a `magnet:` prefix.
///
/// # Errors
/// Returns [`EngineError::InvalidMagnet`] for malformed, oversized or unsupported topics.
pub fn validate_magnet(value: &str) -> Result<(), EngineError> {
    if value.len() > 16 * 1024 || value.chars().any(char::is_control) {
        return Err(EngineError::InvalidMagnet);
    }
    let url = Url::parse(value).map_err(|_| EngineError::InvalidMagnet)?;
    if url.scheme() != "magnet"
        || url.host().is_some()
        || !url.path().is_empty()
        || url.fragment().is_some()
    {
        return Err(EngineError::InvalidMagnet);
    }
    let mut valid = false;
    for (_, topic) in url
        .query_pairs()
        .filter(|(name, _)| name.eq_ignore_ascii_case("xt"))
    {
        let lower = topic.to_ascii_lowercase();
        let good = if let Some(hash) = lower.strip_prefix("urn:btih:") {
            (hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
                || (hash.len() == 32
                    && hash.bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7')))
        } else if let Some(hash) = lower.strip_prefix("urn:btmh:1220") {
            hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit())
        } else {
            false
        };
        if !good {
            return Err(EngineError::InvalidMagnet);
        }
        valid = true;
    }
    if valid {
        Ok(())
    } else {
        Err(EngineError::InvalidMagnet)
    }
}

impl CardigannExecutor {
    /// Whether a trusted search row has a policy-allowed resolution target.
    /// HTTP success and metadata validation are checked when it is consumed.
    pub fn can_resolve_release(&self, release: &SearchResult) -> bool {
        if release.indexer_instance_id != self.indexer_instance_id
            || release.definition_id != self.definition.metadata.id
        {
            return false;
        }
        if let Some(magnet) = &release.magnet_url {
            return validate_magnet(magnet).is_ok();
        }
        let target = release.download_url.as_ref().or_else(|| {
            self.definition
                .spec
                .implementation
                .download
                .as_ref()
                .and(release.details_url.as_ref())
        });
        target.is_some_and(|target| {
            if target.starts_with("magnet:") {
                validate_magnet(target).is_ok()
            } else {
                Url::parse(target).is_ok_and(|url| self.ensure_allowed(&url).is_ok())
            }
        })
    }

    /// Resolves a release using this configured instance's authentication,
    /// definition and mirror policy. The caller must retain trusted search rows;
    /// never deserialize an arbitrary client URL into this input.
    ///
    /// # Errors
    /// Returns an execution error for instance mismatch, authentication, unsafe
    /// URLs, exhausted budgets, failed transport or invalid download metadata.
    pub async fn resolve_release(
        &self,
        release: &SearchResult,
    ) -> Result<DownloadArtifact, EngineError> {
        if release.indexer_instance_id != self.indexer_instance_id
            || release.definition_id != self.definition.metadata.id
        {
            return Err(EngineError::ReleaseIndexerMismatch);
        }
        if release.protocol == indexloom_core::ReleaseProtocol::Usenet
            && matches!(
                self.definition.spec.implementation.engine,
                indexloom_definitions::DefinitionEngine::Newznab
            )
        {
            return Ok(DownloadArtifact::Nzb(self.download_newznab(release).await?));
        }
        if let Some(magnet) = &release.magnet_url {
            validate_magnet(magnet)?;
            return Ok(DownloadArtifact::Magnet(magnet.clone()));
        }
        let source = release
            .download_url
            .as_ref()
            .or_else(|| {
                self.definition
                    .spec
                    .implementation
                    .download
                    .as_ref()
                    .and(release.details_url.as_ref())
            })
            .ok_or(EngineError::DownloadNotFound)?;
        if source.starts_with("magnet:") {
            validate_magnet(source)?;
            return Ok(DownloadArtifact::Magnet(source.clone()));
        }
        let source = Url::parse(source)?;
        self.ensure_allowed(&source)?;
        self.with_mirrors(|executor| {
            let source = source.clone();
            let release = release.clone();
            Box::pin(async move {
                executor.authenticate_if_needed().await?;
                let target = executor.mirror_release_url(&source)?;
                let mut budget = 24_usize;
                let block = executor
                    .definition
                    .spec
                    .implementation
                    .download
                    .as_ref()
                    .and_then(mapping);
                let render = executor.download_context(&target, &release);
                executor
                    .resolve_download_page(target, block, render, None, 0, &mut budget)
                    .await
            })
        })
        .await
    }

    fn download_context(&self, url: &Url, release: &SearchResult) -> TemplateValue {
        let fields = BTreeMap::from([
            ("title".to_owned(), release.title.clone()),
            (
                "id".to_owned(),
                release.source_id.clone().unwrap_or_default(),
            ),
        ]);
        let mut render = self.context.for_search("", fields);
        let query = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), TemplateValue::String(value.into_owned())))
            .collect();
        if let TemplateValue::Object(root) = &mut render {
            root.insert(
                "DownloadUri".to_owned(),
                TemplateValue::Object(HashMap::from([
                    (
                        "AbsoluteUri".to_owned(),
                        TemplateValue::String(url.to_string()),
                    ),
                    (
                        "AbsolutePath".to_owned(),
                        TemplateValue::String(url.path().to_owned()),
                    ),
                    (
                        "Scheme".to_owned(),
                        TemplateValue::String(url.scheme().to_owned()),
                    ),
                    (
                        "Host".to_owned(),
                        TemplateValue::String(url.host_str().unwrap_or_default().to_owned()),
                    ),
                    ("Query".to_owned(), TemplateValue::Object(query)),
                ])),
            );
        }
        render
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Recursive bounded selector resolution is kept together for clear depth and request accounting."
    )]
    fn resolve_download_page<'a>(
        &'a self,
        target: Url,
        block: Option<&'a Mapping>,
        render: TemplateValue,
        cached: Option<BinaryResponse>,
        depth: usize,
        budget: &'a mut usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<DownloadArtifact, EngineError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if depth >= 6 {
                return Err(EngineError::DownloadBudgetExceeded);
            }
            let search = mapping(&self.definition.spec.implementation.search);
            let headers = block
                .and_then(|b| b.get(key("headers")))
                .or_else(|| search.and_then(|s| s.get(key("headers"))));
            let headers = render_headers(headers, &render)?;
            let method = download_method(block.and_then(|b| b.get(key("method"))), &render)?;
            let inputs = render_inputs(block.and_then(|b| b.get(key("inputs"))), &render, true)?;
            let mut page = cached;
            let mut before_response = None;
            if let Some(before) = block.and_then(|b| b.get(key("before"))).and_then(mapping) {
                let before_path =
                    if let Some(selector) = before.get(key("pathselector")).and_then(mapping) {
                        if page.is_none() {
                            page = Some(
                                self.grab_request(
                                    target.clone(),
                                    Method::GET,
                                    headers.clone(),
                                    Vec::new(),
                                    budget,
                                )
                                .await?,
                            );
                        }
                        select(&page.as_ref().unwrap().body, selector, &render)?
                            .ok_or(EngineError::DownloadNotFound)?
                    } else {
                        render_template(
                            &before.get(key("path")).and_then(scalar).ok_or(
                                EngineError::InvalidDownloadDefinition("before.path is missing"),
                            )?,
                            &render,
                        )?
                    };
                // Static before paths are relative to the selected site; dynamic
                // path selectors are relative to the response that supplied them.
                let base = if before.contains_key(key("pathselector")) {
                    &page.as_ref().unwrap().final_url
                } else {
                    &self.base_url
                };
                let url = self.resolve_url(base, &before_path)?;
                let before_headers = if before.contains_key(key("headers")) {
                    render_headers(before.get(key("headers")), &render)?
                } else {
                    headers.clone()
                };
                before_response = Some(
                    self.grab_request(
                        url,
                        download_method(before.get(key("method")), &render)?,
                        before_headers,
                        render_inputs(before.get(key("inputs")), &render, true)?,
                        budget,
                    )
                    .await?,
                );
            }
            if let Some(info) = block.and_then(|b| b.get(key("infohash"))).and_then(mapping) {
                let response = self
                    .download_source(
                        &target,
                        &headers,
                        info,
                        &mut page,
                        before_response.as_ref(),
                        budget,
                    )
                    .await?;
                let hash = info.get(key("hash")).and_then(mapping).ok_or(
                    EngineError::InvalidDownloadDefinition("infohash.hash is missing"),
                )?;
                let title = info.get(key("title")).and_then(mapping).ok_or(
                    EngineError::InvalidDownloadDefinition("infohash.title is missing"),
                )?;
                let hash =
                    select(&response.body, hash, &render)?.ok_or(EngineError::DownloadNotFound)?;
                let title =
                    select(&response.body, title, &render)?.ok_or(EngineError::DownloadNotFound)?;
                let hash =
                    super::extract::normalize_info_hash(&hash).ok_or(EngineError::InvalidMagnet)?;
                let mut magnet = Url::parse("magnet:")?;
                magnet
                    .query_pairs_mut()
                    .append_pair("xt", &format!("urn:btih:{hash}"))
                    .append_pair("dn", &title);
                validate_magnet(magnet.as_str())?;
                return Ok(DownloadArtifact::Magnet(magnet.to_string()));
            }
            if let Some(selectors) = block
                .and_then(|b| b.get(key("selectors")))
                .and_then(YamlValue::as_sequence)
            {
                let mut candidate_error = None;
                for selector in selectors {
                    let selector = mapping(selector).ok_or(
                        EngineError::InvalidDownloadDefinition("selector must be a mapping"),
                    )?;
                    let response = self
                        .download_source(
                            &target,
                            &headers,
                            selector,
                            &mut page,
                            before_response.as_ref(),
                            budget,
                        )
                        .await?;
                    let Some(href) = select(&response.body, selector, &render)? else {
                        continue;
                    };
                    if href.starts_with("magnet:") {
                        validate_magnet(&href)?;
                        return Ok(DownloadArtifact::Magnet(href));
                    }
                    let next = self.resolve_url(&response.final_url, &href)?;
                    let fetched = match self
                        .grab_request(
                            next.clone(),
                            method.clone(),
                            headers.clone(),
                            inputs.clone(),
                            budget,
                        )
                        .await
                    {
                        Ok(fetched) => fetched,
                        Err(EngineError::UnexpectedStatus(404 | 410)) => continue,
                        Err(error) if super::mirrors::retryable(&error) => {
                            candidate_error = Some(error);
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    if let Ok(artifact) = artifact(&fetched) {
                        return Ok(artifact);
                    }
                    // A nested page can describe its own download mapping. For
                    // ordinary Cardigann selectors, apply the ordered selectors
                    // again with a strict depth/request budget.
                    let explicit_nested = selector.get(key("download")).and_then(mapping);
                    let mut nested = explicit_nested.or(block).cloned();
                    if explicit_nested.is_none()
                        && let Some(nested) = &mut nested
                    {
                        // The parent's before request has already run. Reusing
                        // its selectors must not replay that side effect.
                        nested.remove(key("before"));
                    }
                    match self
                        .resolve_download_page(
                            next,
                            nested.as_ref(),
                            render.clone(),
                            Some(fetched),
                            depth + 1,
                            budget,
                        )
                        .await
                    {
                        Ok(artifact) => return Ok(artifact),
                        Err(EngineError::DownloadNotFound | EngineError::InvalidTorrent) => {}
                        Err(error) if super::mirrors::retryable(&error) => {
                            candidate_error = Some(error);
                        }
                        Err(error) => return Err(error),
                    }
                }
                return Err(candidate_error.unwrap_or(EngineError::DownloadNotFound));
            }
            let response = if let Some(cached) = page {
                cached
            } else {
                self.grab_request(target, method, headers, inputs, budget)
                    .await?
            };
            artifact(&response)
        })
    }

    async fn download_source<'a>(
        &self,
        target: &Url,
        headers: &[(HeaderName, String)],
        rule: &Mapping,
        page: &'a mut Option<BinaryResponse>,
        before: Option<&'a BinaryResponse>,
        budget: &mut usize,
    ) -> Result<&'a BinaryResponse, EngineError> {
        if rule
            .get(key("usebeforeresponse"))
            .and_then(YamlValue::as_bool)
            .unwrap_or(false)
            && let Some(before) = before
        {
            return Ok(before);
        }
        if page.is_none() {
            *page = Some(
                self.grab_request(
                    target.clone(),
                    Method::GET,
                    headers.to_vec(),
                    Vec::new(),
                    budget,
                )
                .await?,
            );
        }
        Ok(page.as_ref().unwrap())
    }

    async fn grab_request(
        &self,
        url: Url,
        method: Method,
        headers: Vec<(HeaderName, String)>,
        inputs: Vec<(String, String)>,
        budget: &mut usize,
    ) -> Result<BinaryResponse, EngineError> {
        if *budget == 0 {
            return Err(EngineError::DownloadBudgetExceeded);
        }
        *budget -= 1;
        self.execute_binary(
            RequestSpec {
                url,
                method,
                headers,
                inputs,
                raw_query: None,
            },
            true,
        )
        .await
    }
}

fn download_method(
    value: Option<&YamlValue>,
    render: &TemplateValue,
) -> Result<Method, EngineError> {
    match render_template(
        &value.and_then(scalar).unwrap_or_else(|| "get".to_owned()),
        render,
    )?
    .to_ascii_lowercase()
    .as_str()
    {
        "get" => Ok(Method::GET),
        "post" => Ok(Method::POST),
        _ => Err(EngineError::InvalidDownloadDefinition(
            "only GET and POST are supported",
        )),
    }
}

fn select(
    body: &[u8],
    rule: &Mapping,
    render: &TemplateValue,
) -> Result<Option<String>, EngineError> {
    let document = Html::parse_document(&String::from_utf8_lossy(body));
    let mut value = if let Some(text) = rule.get(key("text")).and_then(scalar) {
        render_template(&text, render)?
    } else {
        let css = render_template(
            &rule
                .get(key("selector"))
                .and_then(scalar)
                .ok_or(EngineError::InvalidDownloadDefinition("missing selector"))?,
            render,
        )?;
        let (css, contains) = normalize_contains_selector(&css);
        let selector = Selector::parse(&css)
            .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
        let Some(element) = document.select(&selector).find(|e| {
            contains
                .iter()
                .all(|needle| element_text(*e).contains(needle))
        }) else {
            return Ok(None);
        };
        if let Some(attribute) = rule.get(key("attribute")).and_then(scalar) {
            let Some(value) = element.value().attr(&attribute) else {
                return Ok(None);
            };
            value.to_owned()
        } else {
            element_text(element)
        }
    };
    if let Some(filters) = rule.get(key("filters")) {
        value = apply_filters(value, filters)?;
    }
    let value = value.trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

fn artifact(response: &BinaryResponse) -> Result<DownloadArtifact, EngineError> {
    if response.final_url.scheme() == "magnet" {
        validate_magnet(response.final_url.as_str())?;
        return Ok(DownloadArtifact::Magnet(response.final_url.to_string()));
    }
    if let Ok(text) = std::str::from_utf8(&response.body)
        && text.trim().starts_with("magnet:")
    {
        validate_magnet(text.trim())?;
        return Ok(DownloadArtifact::Magnet(text.trim().to_owned()));
    }
    validate_torrent(&response.body)?;
    Ok(DownloadArtifact::Torrent(response.body.clone()))
}

#[cfg(test)]
mod tests;
