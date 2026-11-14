mod challenge;
pub use challenge::{LoginChallengeDescriptor, LoginField, LoginProgress};
pub mod download;
mod extract;
mod extract_html;
mod extract_json;
mod filters;
mod http;
mod mirrors;
mod newznab;
mod rows;
mod templates;
use templates::{
    all_search_paths, path_matches_categories, render_headers, render_inputs, render_raw_query,
    render_search_inputs, render_template, resolve_category_ids, retryable_search_error,
    set_pagination,
};
mod context;
mod dates;
mod response;
use context::RenderContext;
mod jsonpath;
mod selectors;
mod transform;
mod transport;
mod xml;
use extract::{parse_html_results, parse_json_results};

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

use filters::{apply_filters, normalize_contains_selector};
use filters::{
    check_error_selectors, configuration_json, key, mapping, parse_date, parse_size, parse_u32,
    scalar, selector_matches,
};

use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use fancy_regex::{Regex as FancyRegex, RegexBuilder as FancyRegexBuilder};
use gtmpl::{Context, FuncError, Template, Value as TemplateValue};
use indexloom_core::SearchResult;
use indexloom_definitions::{ConfigurationValue, IndexerDefinition};
use regex::Regex;
use reqwest::{Method, StatusCode, header::HeaderName};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value as YamlValue};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::{
    DirectRequestProfile, EngineError, FlareSolverrClient, ProxyRoute, SearchEngine,
    SessionPersistence, validate_public_url_literal,
};

const MAX_DIRECT_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const AUTH_SESSION_TTL: Duration = Duration::from_mins(15);
const CARDIGANN_REGEX_BACKTRACK_LIMIT: usize = 1_000_000;
static HYPHENATED_CONFIG_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.Config\.([A-Za-z0-9_]+(?:-[A-Za-z0-9_]+)+)")
        .expect("static Cardigann config-reference regex must compile")
});

/// Structured search fields supported by Cardigann templates.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchParameters {
    pub mode: Option<String>,
    pub category: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub episode: Option<u16>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub tvdb_id: Option<String>,
    pub tvmaze_id: Option<String>,
    pub trakt_id: Option<String>,
    pub douban_id: Option<String>,
    pub tvrage_id: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub author: Option<String>,
    pub title: Option<String>,
    pub publisher: Option<String>,
    pub label: Option<String>,
    pub track: Option<String>,
    pub group: Option<String>,
    pub episode_code: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub min_seeders: Option<u32>,
    pub max_age_days: Option<u32>,
}

/// Fully resolved, execution-only configuration for one indexer instance.
pub struct IndexerExecutionConfig {
    pub indexer_instance_id: String,
    pub definition: IndexerDefinition,
    pub values: BTreeMap<String, JsonValue>,
    pub secrets: BTreeMap<String, String>,
    pub user_agent: Option<String>,
    pub proxy: Option<ProxyRoute>,
    pub flaresolverr: Option<FlareSolverrClient>,
}

impl std::fmt::Debug for IndexerExecutionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndexerExecutionConfig")
            .field("indexer_instance_id", &self.indexer_instance_id)
            .field("definition", &self.definition.metadata.id)
            .field("values", &self.values)
            .field("secret_names", &self.secrets.keys().collect::<Vec<_>>())
            .field("user_agent", &self.user_agent)
            .field("proxy", &self.proxy)
            .field("flaresolverr", &self.flaresolverr)
            .finish()
    }
}

/// Results returned by one indexer, preserving its boundary for grouped UI.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerSearchBatch {
    pub indexer_instance_id: String,
    pub definition_id: String,
    pub indexer_name: String,
    pub response_time_ms: u64,
    pub results: Vec<SearchResult>,
}

/// Connectivity/authentication test outcome.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerTestOutcome {
    pub indexer_id: String,
    pub indexer_name: String,
    pub success: bool,
    pub response_time_ms: u64,
    pub message: String,
}

/// Cardigann-compatible executor. Every instance owns a separate reqwest cookie
/// store so private tracker sessions can never bleed into another indexer.
#[derive(Debug)]
pub struct CardigannExecutor {
    indexer_instance_id: String,
    definition: IndexerDefinition,
    context: RenderContext,
    engine: SearchEngine,
    profile: DirectRequestProfile,
    proxy: Option<ProxyRoute>,
    flaresolverr: Option<FlareSolverrClient>,
    base_url: Url,
    allowed_origins: Vec<Origin>,
    auth_state: Arc<tokio::sync::Mutex<AuthSessionState>>,
    mirrors: Arc<mirrors::MirrorPool>,
    #[cfg(test)]
    transport: Option<Arc<dyn http::MockTransport>>,
    encoding: transport::WireEncoding,
    request_gate: Arc<transport::RequestGate>,
    operation: Arc<tokio::sync::Mutex<()>>,
    persistence: Option<Arc<dyn SessionPersistence>>,
    configured_cookie: bool,
}

impl CardigannExecutor {
    /// Builds an executor from a validated definition and decrypted settings.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for invalid catalog URLs or proxy configuration.
    pub fn new(config: IndexerExecutionConfig) -> Result<Self, EngineError> {
        let roots =
            transport::certificate_roots(&config.definition.spec.implementation.certificates)?;
        if !roots.is_empty() && config.flaresolverr.is_some() {
            return Err(EngineError::CardigannIncompatible(
                "custom CA roots cannot be enforced through FlareSolverr",
            ));
        }
        let encoding =
            transport::WireEncoding::new(&config.definition.spec.implementation.encoding)?;
        let request_gate = Arc::new(transport::RequestGate::new(
            config.definition.spec.implementation.request_delay_seconds,
        )?);
        let base_url = Url::parse(
            config
                .definition
                .spec
                .base_urls
                .first()
                .ok_or(EngineError::MissingBaseUrl)?,
        )?;
        validate_public_url_literal(&base_url)?;
        let allowed_origins = config
            .definition
            .spec
            .base_urls
            .iter()
            .chain(config.definition.spec.legacy_urls.iter())
            .map(|value| {
                let url = Url::parse(value)?;
                validate_public_url_literal(&url)?;
                Ok::<Origin, EngineError>(Origin::from_url(&url))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mirrors = Arc::new(mirrors::MirrorPool::new(
            &config.definition,
            config.proxy.as_ref(),
        )?);
        let engine = SearchEngine::with_proxy_and_roots(config.proxy.as_ref(), roots)?;
        let cookie_header = config
            .secrets
            .iter()
            .find(|(name, _)| name.to_ascii_lowercase().contains("cookie"))
            .map(|(_, value)| value.clone());
        let configured_cookie = cookie_header.is_some();
        if let Some(header) = cookie_header.as_deref() {
            engine.cookies.seed(header, &base_url);
        }
        let mut values = config.values;
        for field in config.definition.configuration_schema().fields {
            if values.contains_key(&field.name) || config.secrets.contains_key(&field.name) {
                continue;
            }
            if let Some(value) = field.default_value {
                values.insert(field.name, configuration_json(value));
            }
        }
        values.insert(
            "sitelink".to_owned(),
            JsonValue::String(base_url.to_string()),
        );
        if let Some(user_agent) = config.user_agent.as_ref() {
            values.insert(
                "useragent".to_owned(),
                JsonValue::String(user_agent.clone()),
            );
        }
        let context = RenderContext::new(values, config.secrets);
        Ok(Self {
            indexer_instance_id: config.indexer_instance_id,
            definition: config.definition,
            context,
            engine,
            profile: DirectRequestProfile {
                user_agent: config.user_agent,
                cookie_header: None,
            },
            proxy: config.proxy,
            flaresolverr: config.flaresolverr,
            base_url,
            allowed_origins,
            auth_state: Arc::new(tokio::sync::Mutex::new(AuthSessionState::default())),
            encoding,
            request_gate,
            operation: Arc::new(tokio::sync::Mutex::new(())),
            persistence: None,
            configured_cookie,
            mirrors,
            #[cfg(test)]
            transport: None,
        })
    }

    /// Attaches the host's per-instance encrypted session backend. Loading is
    /// lazy, before the first authentication; completed operations write through.
    #[must_use]
    pub fn with_session_persistence(mut self, persistence: Arc<dyn SessionPersistence>) -> Self {
        self.persistence = Some(persistence);
        self
    }

    async fn persist_session(&self) -> Result<(), EngineError> {
        if let Some(persistence) = &self.persistence {
            match self.engine.cookies.snapshot()? {
                Some(snapshot) => persistence.save(snapshot).await?,
                None => persistence.clear().await?,
            }
        }
        Ok(())
    }

    async fn revoke_session(&self) -> Result<(), EngineError> {
        self.engine.cookies.clear();
        self.auth_state.lock().await.last_success = None;
        if let Some(persistence) = &self.persistence {
            persistence.clear().await?;
        }
        Ok(())
    }

    /// Tests transport and, when described by the definition, authentication.
    pub async fn test(&self) -> IndexerTestOutcome {
        let started = Instant::now();
        let result = self.test_mirrors().await;
        IndexerTestOutcome {
            indexer_id: self.indexer_instance_id.clone(),
            indexer_name: self.definition.metadata.name.clone(),
            success: result.is_ok(),
            response_time_ms: elapsed_millis(started),
            message: match result {
                Ok(message) => message,
                Err(error) => error.to_string(),
            },
        }
    }

    async fn test_single(&self) -> Result<String, EngineError> {
        let _operation = self.operation.lock().await;
        let mut result = self.test_inner().await;
        if result.is_ok() {
            if let Err(error) = self.persist_session().await {
                result = Err(error);
            }
        } else if let Err(error) = self.revoke_session().await {
            result = Err(error);
        }
        result
    }

    /// Searches one indexer and returns its results as a distinct batch.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for template, authentication, transport, or
    /// extraction failures.
    pub async fn search(
        &self,
        term: &str,
        limit: usize,
    ) -> Result<IndexerSearchBatch, EngineError> {
        self.search_with_parameters(term, limit, &SearchParameters::default())
            .await
    }

    /// Searches using structured Cardigann fields when declared by the tracker.
    ///
    /// # Errors
    ///
    /// Returns an execution error when requests or extractor rules fail.
    pub async fn search_with_parameters(
        &self,
        term: &str,
        limit: usize,
        parameters: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        self.search_page_with_parameters(term, limit, 0, parameters)
            .await
    }

    /// Executes all applicable definition paths for one requested offset.
    /// Pagination is explicit: no speculative page crawling or unbounded requests.
    ///
    /// # Errors
    /// Returns a definition, transport, or parsing error.
    pub async fn search_page_with_parameters(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
        parameters: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        self.search_page_mirrors(term, limit, offset, parameters)
            .await
    }

    async fn search_page_single(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
        parameters: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        if term.trim().is_empty() {
            return Err(EngineError::EmptySearchTerm);
        }
        let _operation = self.operation.lock().await;
        let mut result = self.search_inner(term, limit, offset, parameters).await;
        if matches!(
            result,
            Err(EngineError::AuthenticationRejected | EngineError::UnexpectedStatus(401 | 403))
        ) {
            self.revoke_session().await?;
            result = self.search_inner(term, limit, offset, parameters).await;
        }
        // Also save deletion/rotation cookies if extraction failed after a valid
        // HTTP response. Never preserve a rejected authentication session.
        if matches!(
            result,
            Err(EngineError::AuthenticationRejected | EngineError::UnexpectedStatus(401 | 403))
        ) {
            self.revoke_session().await?;
        } else {
            let mut state = self.auth_state.lock().await;
            let trusted = self.definition.spec.implementation.authentication.is_none()
                || state.last_success.is_some();
            // Rotation can shorten a cookie's deadline. Recheck authentication
            // at that new deadline even while the 15-minute cache is warm.
            state.cookie_expiry = self.engine.cookies.earliest_expiry();
            drop(state);
            if trusted {
                self.persist_session().await?;
            } else {
                self.revoke_session().await?;
            }
        }
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Keep request aggregation and partial failure policy in one place."
    )]
    async fn search_inner(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
        parameters: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        if matches!(
            self.definition.spec.implementation.engine,
            indexloom_definitions::DefinitionEngine::Newznab
        ) {
            return self.search_newznab(term, limit, offset, parameters).await;
        }
        if term.trim().is_empty()
            && ![
                parameters.imdb_id.as_deref(),
                parameters.tmdb_id.as_deref(),
                parameters.tvdb_id.as_deref(),
                parameters.tvmaze_id.as_deref(),
                parameters.trakt_id.as_deref(),
                parameters.douban_id.as_deref(),
                parameters.tvrage_id.as_deref(),
                parameters.artist.as_deref(),
                parameters.album.as_deref(),
                parameters.author.as_deref(),
                parameters.title.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|v| !v.trim().is_empty())
        {
            return Err(EngineError::EmptySearchTerm);
        }
        let started = Instant::now();
        self.authenticate_if_needed().await?;
        let search = mapping(&self.definition.spec.implementation.search).ok_or(
            EngineError::InvalidSearchDefinition("search must be a mapping"),
        )?;
        let keywords = search
            .get(key("keywordsfilters"))
            .map(|filters| {
                transform::apply(
                    term.to_owned(),
                    filters,
                    &self.context.for_search(term, BTreeMap::new()),
                    &self.definition.spec.implementation.encoding,
                )
            })
            .transpose()?
            .unwrap_or_else(|| term.to_owned());
        if let Some(mode) = parameters.mode.as_deref() {
            let supported = mapping(&self.definition.spec.implementation.capabilities)
                .and_then(|caps| caps.get(key("modes")))
                .and_then(mapping)
                .is_some_and(|modes| modes.contains_key(mode));
            if !supported {
                return Err(EngineError::UnsupportedSearchMode(mode.to_owned()));
            }
        }
        let categories = resolve_category_ids(&self.definition, parameters.category.as_deref());
        if let Some(category) = parameters.category.as_deref()
            && categories.is_empty()
        {
            return Err(EngineError::UnsupportedSearchCategory(category.to_owned()));
        }
        let mut execution_context = self.context.clone();
        execution_context.parameters = parameters.clone();
        execution_context.categories.clone_from(&categories);
        execution_context.limit = limit.clamp(1, 500);
        execution_context.offset = offset;
        execution_context.skip_andmatch =
            templates::supports_id_search(&self.definition, parameters);
        let paths = all_search_paths(search)?;
        if paths.len() > 64 {
            return Err(EngineError::CardigannIncompatible(
                "more than 64 search paths",
            ));
        }
        let mut results = Vec::new();
        let mut last_error = None;
        let mut completed = 0;
        let mut requested = std::collections::BTreeSet::new();
        for path in paths {
            if !path_matches_categories(path, &categories)? {
                continue;
            }
            let mut path_context = execution_context.clone();
            if let Some(restrictions) = path.get(key("categories")).and_then(YamlValue::as_array) {
                let allowed = restrictions.iter().filter_map(scalar).collect::<Vec<_>>();
                if allowed.first().is_none_or(|v| v != "!") && !categories.is_empty() {
                    path_context.categories.retain(|v| allowed.contains(v));
                }
            }
            let render = path_context.for_search(&keywords, BTreeMap::new());
            // Deduplicate identical rendered requests while preserving explicit offsets.
            let signature = render_template(
                &serde_yaml::to_string(path)
                    .map_err(|_| EngineError::InvalidSearchDefinition("invalid search path"))?,
                &render,
            )?;
            if !requested.insert(signature) {
                continue;
            }
            let response = match self.execute_search_path(search, path, &render).await {
                Ok(response) => response,
                Err(error) if retryable_search_error(&error) => {
                    last_error = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            completed += 1;
            let parsed =
                self.parse_search_response(search, path, response, &path_context, &keywords)?;
            results.extend(
                parsed
                    .into_iter()
                    .take(500_usize.saturating_sub(results.len())),
            );
        }
        if completed == 0
            && let Some(error) = last_error
        {
            return Err(error);
        }
        let mut identities = std::collections::BTreeSet::new();
        results.retain(|r| {
            identities.insert((
                r.title.clone(),
                r.download_url.clone(),
                r.magnet_url.clone(),
                r.details_url.clone(),
            ))
        });
        results.truncate(limit.clamp(1, 500));
        Ok(IndexerSearchBatch {
            indexer_instance_id: self.indexer_instance_id.clone(),
            definition_id: self.definition.metadata.id.clone(),
            indexer_name: self.definition.metadata.name.clone(),
            response_time_ms: elapsed_millis(started),
            results,
        })
    }

    async fn test_inner(&self) -> Result<String, EngineError> {
        if matches!(
            self.definition.spec.implementation.engine,
            indexloom_definitions::DefinitionEngine::Newznab
        ) {
            return self.test_newznab().await;
        }
        self.authenticate_if_needed().await?;
        if self.definition.spec.implementation.authentication.is_some() {
            return Ok("authentication and tracker test succeeded".to_owned());
        }
        let response = self
            .execute_direct(RequestSpec {
                method: Method::GET,
                url: self.base_url.clone(),
                headers: Vec::new(),
                inputs: Vec::new(),
                raw_query: None,
            })
            .await?;
        if response.status.is_success() || response.status.is_redirection() {
            Ok(format!("tracker responded with HTTP {}", response.status))
        } else {
            Err(EngineError::UnexpectedStatus(response.status.as_u16()))
        }
    }

    async fn authenticate_if_needed(&self) -> Result<(), EngineError> {
        let mut state = self.auth_state.lock().await;
        let mut restored = false;
        if !state.loaded {
            if let Some(persistence) = &self.persistence
                && let Some(snapshot) = persistence.load().await?
            {
                self.engine.cookies.restore(&snapshot)?;
                restored = self.engine.cookies.has_cookies();
            }
            state.loaded = true;
        }
        let Some(authentication) = self
            .definition
            .spec
            .implementation
            .authentication
            .as_ref()
            .and_then(mapping)
        else {
            return Ok(());
        };
        if state.revoked {
            return Err(EngineError::LoginChallengeUnavailable);
        }
        state.interactive.prune();
        if state.interactive.pending.is_some() {
            return Err(EngineError::LoginChallengeRequired);
        }
        if state
            .last_success
            .is_some_and(|at| at.elapsed() < AUTH_SESSION_TTL)
            && self.engine.cookies.has_cookies()
            && state
                .cookie_expiry
                .is_none_or(|at| at > OffsetDateTime::now_utc().unix_timestamp())
        {
            return Ok(());
        }
        // Revalidate restored cookies using the definition's test before login.
        // Without a test, the next request checks the restored session; 401/403
        // triggers a single fresh login.
        if restored {
            match self.authenticate(authentication, true).await {
                Ok(()) => {
                    state.last_success = Some(Instant::now());
                    state.cookie_expiry = self.engine.cookies.earliest_expiry();
                    return self.persist_session().await;
                }
                Err(
                    EngineError::AuthenticationRejected | EngineError::UnexpectedStatus(401 | 403),
                ) => {
                    self.engine.cookies.clear();
                    if let Some(persistence) = &self.persistence {
                        persistence.clear().await?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        self.authenticate(authentication, false).await?;
        state.last_success = Some(Instant::now());
        state.cookie_expiry = self.engine.cookies.earliest_expiry();
        self.persist_session().await
    }

    async fn authenticate(
        &self,
        authentication: &Mapping,
        restored: bool,
    ) -> Result<(), EngineError> {
        let configured_cookie = self.configured_cookie || restored;
        if authentication.contains_key(key("captcha")) && !configured_cookie {
            return Err(EngineError::LoginChallengeRequired);
        }
        let render = self.context.for_search("", BTreeMap::new());
        let method = authentication
            .get(key("method"))
            .and_then(scalar)
            .unwrap_or_else(|| "get".to_owned())
            .to_ascii_lowercase();
        let path = authentication
            .get(key("path"))
            .and_then(scalar)
            .unwrap_or_default();
        // A manually supplied cookie is an authenticated session snapshot. Do
        // not submit the login form again; only run the definition's auth test.
        if !path.is_empty() && !configured_cookie {
            let target = self.render_url(&path, &render)?;
            let inputs = render_inputs(authentication.get(key("inputs")), &render, false)?;
            let headers = render_headers(authentication.get(key("headers")), &render)?;
            let response = match method.as_str() {
                "get" => {
                    self.execute_direct(RequestSpec {
                        method: Method::GET,
                        url: target,
                        headers,
                        inputs,
                        raw_query: None,
                    })
                    .await?
                }
                "post" => {
                    self.execute_direct(RequestSpec {
                        method: Method::POST,
                        url: target,
                        headers,
                        inputs,
                        raw_query: None,
                    })
                    .await?
                }
                "form" => {
                    self.execute_form_login(authentication, target, inputs, headers)
                        .await?
                }
                other => {
                    return Err(EngineError::UnsupportedAuthentication(format!(
                        "unsupported authentication method {other}"
                    )));
                }
            };
            check_error_selectors(authentication.get(key("error")), &response.body)?;
        }
        if let Some(test) = authentication.get(key("test")).and_then(mapping) {
            let path = test.get(key("path")).and_then(scalar).unwrap_or_default();
            let target = self.render_url(&path, &render)?;
            let response = self
                .execute_direct(RequestSpec {
                    method: Method::GET,
                    url: target,
                    headers: Vec::new(),
                    inputs: Vec::new(),
                    raw_query: None,
                })
                .await?;
            if let Some(selector) = test.get(key("selector")).and_then(scalar)
                && !selector_matches(&response.body, &selector)?
            {
                return Err(EngineError::AuthenticationRejected);
            }
        }
        Ok(())
    }

    async fn execute_form_login(
        &self,
        authentication: &Mapping,
        login_url: Url,
        mut inputs: Vec<(String, String)>,
        headers: Vec<(HeaderName, String)>,
    ) -> Result<ExecutionResponse, EngineError> {
        let login_page = self
            .execute_direct(RequestSpec {
                method: Method::GET,
                url: login_url,
                headers: headers.clone(),
                inputs: Vec::new(),
                raw_query: None,
            })
            .await?;
        if challenge::requires_interactive(authentication, &login_page.body)? {
            return Err(EngineError::LoginChallengeRequired);
        }
        let (target, form_values) = {
            let selector_text = authentication
                .get(key("form"))
                .and_then(scalar)
                .unwrap_or_else(|| "form".to_owned());
            let selector = Selector::parse(&selector_text)
                .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
            let document = Html::parse_document(&login_page.body);
            let form = document
                .select(&selector)
                .next()
                .ok_or(EngineError::AuthenticationFormMissing)?;
            let input_selector = Selector::parse("input[name]")
                .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
            let mut form_values = form
                .select(&input_selector)
                .filter_map(|input| {
                    Some((
                        input.value().attr("name")?.to_owned(),
                        input.value().attr("value").unwrap_or_default().to_owned(),
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            form_values.extend(inputs.drain(..));
            let action = form.value().attr("action").unwrap_or("");
            (
                self.resolve_url(&login_page.final_url, action)?,
                form_values,
            )
        };
        self.execute_direct(RequestSpec {
            method: Method::POST,
            url: target,
            headers,
            inputs: form_values.into_iter().collect(),
            raw_query: None,
        })
        .await
    }

    async fn execute_search_path(
        &self,
        search: &Mapping,
        path: &Mapping,
        render: &TemplateValue,
    ) -> Result<ExecutionResponse, EngineError> {
        let path_value =
            path.get(key("path"))
                .and_then(scalar)
                .ok_or(EngineError::InvalidSearchDefinition(
                    "search path is missing",
                ))?;
        let target = self.render_url(&path_value, render)?;
        let method = path
            .get(key("method"))
            .and_then(scalar)
            .unwrap_or_else(|| "get".to_owned());
        let method = render_template(&method, render)?.to_ascii_lowercase();
        let allow_empty = search
            .get(key("allowEmptyInputs"))
            .or_else(|| search.get(key("allowemptyinputs")))
            .and_then(YamlValue::as_bool)
            .unwrap_or(false);
        let inputs = render_search_inputs(search, path, render, allow_empty)?;
        let raw_query = render_raw_query(
            search,
            path,
            render,
            &self.definition.spec.implementation.encoding,
        )?;
        let headers = render_headers(search.get(key("headers")), render)?;
        let response_type = path
            .get(key("response"))
            .and_then(mapping)
            .and_then(|response| response.get(key("type")))
            .and_then(scalar)
            .unwrap_or_else(|| "html".to_owned())
            .to_ascii_lowercase();
        if let Some(flaresolverr) = &self.flaresolverr {
            if method != "get" || !headers.is_empty() {
                return Err(EngineError::FlareSolverrUnsupportedRequest);
            }
            let mut url = target;
            self.encoding
                .query(&mut url, &inputs, raw_query.as_deref())?;
            self.request_gate.wait().await;
            let cookies = self.engine.cookies.browser_cookies(&url);
            let solution = flaresolverr
                .get(&url, &cookies, self.proxy.as_ref())
                .await?;
            self.engine.import_browser_solution(&url, &solution);
            let status = StatusCode::from_u16(solution.status)
                .map_err(|_| EngineError::InvalidResponseStatus(solution.status))?;
            if !status.is_success() {
                return Err(EngineError::UnexpectedStatus(status.as_u16()));
            }
            return Ok(ExecutionResponse {
                status,
                body: solution.response,
                final_url: url,
                response_type,
            });
        }
        let mut response = self
            .execute_direct(RequestSpec {
                method: match method.as_str() {
                    "get" => Method::GET,
                    "post" => Method::POST,
                    other => return Err(EngineError::UnsupportedHttpMethod(other.to_owned())),
                },
                url: target,
                headers,
                inputs,
                raw_query,
            })
            .await?;
        response.response_type = response_type;
        Ok(response)
    }

    #[cfg(test)]
    fn direct_request(&self, spec: RequestSpec) -> Result<reqwest::RequestBuilder, EngineError> {
        let mut url = spec.url;
        if spec.method == Method::GET {
            self.encoding
                .query(&mut url, &spec.inputs, spec.raw_query.as_deref())?;
        }
        self.ensure_allowed(&url)?;
        let mut request = self
            .engine
            .request(spec.method.clone(), url.clone(), &self.profile);
        for (name, value) in &spec.headers {
            if name == reqwest::header::COOKIE {
                return Err(EngineError::InvalidSearchDefinition(
                    "Cookie headers must use the scoped session jar",
                ));
            }
            request = request.header(name, value);
        }
        if spec.method == Method::POST {
            let mut body = self.encoding.form(&spec.inputs)?;
            if let Some(raw) = spec.raw_query.as_deref().filter(|v| !v.is_empty()) {
                if !body.is_empty() {
                    body.push('&');
                }
                body.push_str(raw.trim_start_matches(['?', '&']));
            }
            request = request
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(body);
        }
        Ok(request)
    }

    async fn execute_direct(&self, spec: RequestSpec) -> Result<ExecutionResponse, EngineError> {
        // Search, login and downloads share bounded requests, redirect policy,
        // per-indexer pacing and the same origin-restricted transport.
        let response = self.execute_binary(spec, false).await?;
        Ok(ExecutionResponse {
            status: response.status,
            body: self.encoding.decode(&response.body)?,
            final_url: response.final_url,
            response_type: "html".to_owned(),
        })
    }

    fn render_url(&self, value: &str, context: &TemplateValue) -> Result<Url, EngineError> {
        let rendered = templates::render_encoded_template(
            value,
            context,
            &self.definition.spec.implementation.encoding,
            true,
        )?;
        self.resolve_url(&self.base_url, &rendered)
    }

    fn resolve_url(&self, base: &Url, value: &str) -> Result<Url, EngineError> {
        let target = base.join(value)?;
        self.ensure_allowed(&target)?;
        Ok(target)
    }

    fn ensure_allowed(&self, target: &Url) -> Result<(), EngineError> {
        if !["http", "https"].contains(&target.scheme())
            || !target.username().is_empty()
            || target.password().is_some()
        {
            return Err(EngineError::OutboundUrlRejected(
                "unsupported scheme or URL user information".into(),
            ));
        }
        validate_public_url_literal(target)?;
        let origin = Origin::from_url(target);
        if self.allowed_origins.contains(&origin) {
            Ok(())
        } else {
            Err(EngineError::OutboundUrlRejected(
                target.origin().ascii_serialization(),
            ))
        }
    }
}

#[derive(Clone, Debug)]
struct RequestSpec {
    method: Method,
    url: Url,
    headers: Vec<(HeaderName, String)>,
    inputs: Vec<(String, String)>,
    raw_query: Option<String>,
}

#[derive(Debug)]
struct ExecutionResponse {
    status: StatusCode,
    body: String,
    final_url: Url,
    response_type: String,
}

#[derive(Debug, Default)]
struct AuthSessionState {
    interactive: challenge::InteractiveState,
    revoked: bool,
    loaded: bool,
    cookie_expiry: Option<i64>,
    last_success: Option<Instant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Origin {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl Origin {
    fn from_url(url: &Url) -> Self {
        Self {
            scheme: url.scheme().to_owned(),
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default(),
        }
    }
}

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_tests;

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
