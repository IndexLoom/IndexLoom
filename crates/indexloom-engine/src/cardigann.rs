mod extract;
mod filters;
use extract::{parse_html_results, parse_json_results};
use filters::{
    apply_case, apply_filters, browser_cookies, check_error_selectors, configuration_json,
    element_text, json_template_value, key, mapping, normalize_contains_selector, parse_date,
    parse_size, parse_u32, scalar, selector_matches,
};

use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
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
    BrowserCookie, DirectRequestProfile, EngineError, FlareSolverrClient, ProxyRoute, SearchEngine,
    validate_public_url_literal,
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
#[derive(Clone, Debug, Default, Deserialize)]
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
    auth_state: tokio::sync::Mutex<AuthSessionState>,
}

impl CardigannExecutor {
    /// Builds an executor from a validated definition and decrypted settings.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for invalid catalog URLs or proxy configuration.
    pub fn new(config: IndexerExecutionConfig) -> Result<Self, EngineError> {
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
        let engine = SearchEngine::with_proxy(config.proxy.as_ref())?;
        let cookie_header = config
            .secrets
            .iter()
            .find(|(name, _)| name.to_ascii_lowercase().contains("cookie"))
            .map(|(_, value)| value.clone());
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
                cookie_header,
            },
            proxy: config.proxy,
            flaresolverr: config.flaresolverr,
            base_url,
            allowed_origins,
            auth_state: tokio::sync::Mutex::new(AuthSessionState::default()),
        })
    }

    /// Tests transport and, when described by the definition, authentication.
    pub async fn test(&self) -> IndexerTestOutcome {
        let started = Instant::now();
        let result = self.test_inner().await;
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
        if term.trim().is_empty() {
            return Err(EngineError::EmptySearchTerm);
        }
        let started = Instant::now();
        self.authenticate_if_needed().await?;
        let search = mapping(&self.definition.spec.implementation.search).ok_or(
            EngineError::InvalidSearchDefinition("search must be a mapping"),
        )?;
        let keywords = search
            .get(key("keywordsfilters"))
            .map(|filters| apply_filters(term.to_owned(), filters))
            .transpose()?
            .unwrap_or_else(|| term.to_owned());
        if let Some(mode) = parameters.mode.as_deref() {
            let supported = mapping(&self.definition.spec.implementation.capabilities)
                .and_then(|caps| caps.get(key("modes")))
                .and_then(mapping)
                .is_some_and(|modes| modes.contains_key(YamlValue::String(mode.to_owned())));
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
        let render = self.context.for_search_with_parameters(
            &keywords,
            BTreeMap::new(),
            parameters,
            &categories,
        );
        // Cardigann can declare multiple search paths. Fall back only for
        // temporary transport/server errors; malformed templates, authentication
        // errors and policy violations are not made safe by trying another path.
        let paths = all_search_paths(search)?;
        let mut last_error = None;
        let mut selected = None;
        for (index, path) in paths.iter().enumerate() {
            match self.execute_search_path(search, path, &render).await {
                Ok(response) => {
                    selected = Some(response);
                    break;
                }
                Err(error) if index + 1 < paths.len() && retryable_search_error(&error) => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        let response = selected.ok_or_else(|| {
            last_error.unwrap_or(EngineError::InvalidSearchDefinition(
                "no usable search path",
            ))
        })?;
        let mut results = match response.response_type.as_str() {
            "json" => parse_json_results(
                &self.indexer_instance_id,
                &self.definition,
                search,
                &response.body,
                &self.context,
                &keywords,
                &response.final_url,
            )?,
            "html" | "xml" => parse_html_results(
                &self.indexer_instance_id,
                &self.definition,
                search,
                &response.body,
                &self.context,
                &keywords,
                &response.final_url,
            )?,
            _ => {
                return Err(EngineError::UnsupportedResponseType(response.response_type));
            }
        };
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
        if self.definition.spec.implementation.authentication.is_some() {
            self.authenticate_if_needed().await?;
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
        let mut state = self.auth_state.lock().await;
        if state
            .last_success
            .is_some_and(|authenticated| authenticated.elapsed() < AUTH_SESSION_TTL)
        {
            return Ok(());
        }
        self.authenticate(authentication).await?;
        state.last_success = Some(Instant::now());
        Ok(())
    }

    async fn authenticate(&self, authentication: &Mapping) -> Result<(), EngineError> {
        let configured_cookie = self.profile.cookie_header.is_some();
        if authentication.contains_key(key("captcha")) && !configured_cookie {
            return Err(EngineError::UnsupportedAuthentication(
                "interactive CAPTCHA requires a cookie or future browser-login workflow".to_owned(),
            ));
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
            .and_then(YamlValue::as_bool)
            .unwrap_or(false);
        let inputs = render_search_inputs(search, path, render, allow_empty)?;
        let raw_query = search
            .get(key("inputs"))
            .and_then(mapping)
            .and_then(|inputs| inputs.get(key("$raw")))
            .and_then(scalar)
            .map(|value| render_template(&value, render))
            .transpose()?;
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
            append_query(&mut url, &inputs, raw_query.as_deref());
            let cookies = browser_cookies(self.profile.cookie_header.as_deref());
            let solution = flaresolverr
                .get(&url, &cookies, self.proxy.as_ref())
                .await?;
            return Ok(ExecutionResponse {
                status: StatusCode::from_u16(solution.status)
                    .map_err(|_| EngineError::InvalidResponseStatus(solution.status))?,
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

    async fn execute_direct(&self, spec: RequestSpec) -> Result<ExecutionResponse, EngineError> {
        let mut url = spec.url;
        if spec.method == Method::GET {
            append_query(&mut url, &spec.inputs, spec.raw_query.as_deref());
        }
        self.ensure_allowed(&url)?;
        let mut request = self
            .engine
            .request(spec.method.clone(), url.clone(), &self.profile);
        for (name, value) in &spec.headers {
            request = request.header(name, value);
        }
        if spec.method == Method::POST {
            request = request.form(&spec.inputs);
        }
        let mut response = request.send().await?;
        let mut redirects = 0_usize;
        while response.status().is_redirection()
            && self
                .definition
                .spec
                .implementation
                .follow_redirects
                .unwrap_or(true)
        {
            if redirects >= MAX_REDIRECTS {
                return Err(EngineError::TooManyRedirects);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(EngineError::RedirectWithoutLocation)?;
            url = self.resolve_url(response.url(), location)?;
            self.ensure_allowed(&url)?;
            response = self
                .engine
                .request(Method::GET, url.clone(), &self.profile)
                .send()
                .await?;
            redirects += 1;
        }
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_DIRECT_RESPONSE_BYTES as u64)
        {
            return Err(EngineError::ResponseTooLarge);
        }
        let final_url = response.url().clone();
        // Content-Length is optional/untrusted: enforce the cap before allocating
        // every streamed chunk, including chunked transfer-encoded responses.
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > MAX_DIRECT_RESPONSE_BYTES {
                return Err(EngineError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        if !status.is_success() {
            return Err(EngineError::UnexpectedStatus(status.as_u16()));
        }
        Ok(ExecutionResponse {
            status,
            body,
            final_url,
            response_type: "html".to_owned(),
        })
    }

    fn render_url(&self, value: &str, context: &TemplateValue) -> Result<Url, EngineError> {
        let rendered = render_template(value, context)?;
        self.resolve_url(&self.base_url, &rendered)
    }

    fn resolve_url(&self, base: &Url, value: &str) -> Result<Url, EngineError> {
        let target = base.join(value)?;
        self.ensure_allowed(&target)?;
        Ok(target)
    }

    fn ensure_allowed(&self, target: &Url) -> Result<(), EngineError> {
        validate_public_url_literal(target)?;
        let origin = Origin::from_url(target);
        if self.allowed_origins.contains(&origin) {
            Ok(())
        } else {
            Err(EngineError::OutboundUrlRejected(target.to_string()))
        }
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
struct RenderContext {
    config: HashMap<String, TemplateValue>,
}

impl RenderContext {
    fn new(values: BTreeMap<String, JsonValue>, secrets: BTreeMap<String, String>) -> Self {
        let mut config = values
            .into_iter()
            .map(|(name, value)| (name, json_template_value(value)))
            .collect::<HashMap<_, _>>();
        config.extend(
            secrets
                .into_iter()
                .map(|(name, value)| (name, TemplateValue::String(value))),
        );
        Self { config }
    }

    fn for_search(&self, keywords: &str, result: BTreeMap<String, String>) -> TemplateValue {
        self.for_search_with_parameters(keywords, result, &SearchParameters::default(), &[])
    }

    fn for_search_with_parameters(
        &self,
        keywords: &str,
        result: BTreeMap<String, String>,
        parameters: &SearchParameters,
        categories: &[String],
    ) -> TemplateValue {
        let mut query = HashMap::from([
            ("Album".to_owned(), TemplateValue::String(String::new())),
            ("Artist".to_owned(), TemplateValue::String(String::new())),
            ("Author".to_owned(), TemplateValue::String(String::new())),
            ("DoubanID".to_owned(), TemplateValue::String(String::new())),
            ("Season".to_owned(), TemplateValue::String(String::new())),
            ("Ep".to_owned(), TemplateValue::String(String::new())),
            ("Genre".to_owned(), TemplateValue::String(String::new())),
            ("IMDBID".to_owned(), TemplateValue::String(String::new())),
            (
                "IMDBIDShort".to_owned(),
                TemplateValue::String(String::new()),
            ),
            ("TMDBID".to_owned(), TemplateValue::String(String::new())),
            ("TVDBID".to_owned(), TemplateValue::String(String::new())),
            ("Title".to_owned(), TemplateValue::String(String::new())),
            ("Type".to_owned(), TemplateValue::String(String::new())),
            ("Year".to_owned(), TemplateValue::String(String::new())),
            ("id".to_owned(), TemplateValue::String(String::new())),
        ]);
        query.insert(
            "Keywords".to_owned(),
            TemplateValue::String(keywords.to_owned()),
        );
        for (field, value) in [
            ("Genre", parameters.genre.as_deref()),
            ("IMDBID", parameters.imdb_id.as_deref()),
            ("TMDBID", parameters.tmdb_id.as_deref()),
            ("Type", parameters.mode.as_deref()),
        ] {
            if let Some(value) = value {
                query.insert(field.to_owned(), TemplateValue::String(value.to_owned()));
            }
        }
        for (field, value) in [
            ("Year", parameters.year),
            ("Season", parameters.season),
            ("Ep", parameters.episode),
        ] {
            if let Some(value) = value {
                query.insert(field.to_owned(), TemplateValue::String(value.to_string()));
            }
        }
        let result = result
            .into_iter()
            .map(|(key, value)| (key, TemplateValue::String(value)))
            .collect();
        TemplateValue::Object(HashMap::from([
            (
                "Config".to_owned(),
                TemplateValue::Object(self.config.clone()),
            ),
            (
                "Keywords".to_owned(),
                TemplateValue::String(keywords.to_owned()),
            ),
            (
                "keywords".to_owned(),
                TemplateValue::String(keywords.to_owned()),
            ),
            ("Query".to_owned(), TemplateValue::Object(query)),
            ("Result".to_owned(), TemplateValue::Object(result)),
            (
                "Categories".to_owned(),
                TemplateValue::Array(
                    categories
                        .iter()
                        .map(|value| TemplateValue::String(value.clone()))
                        .collect(),
                ),
            ),
            ("True".to_owned(), TemplateValue::Bool(true)),
            ("False".to_owned(), TemplateValue::Bool(false)),
            ("Today".to_owned(), TemplateValue::String(String::new())),
        ]))
    }
}

fn render_template(source: &str, context: &TemplateValue) -> Result<String, EngineError> {
    if !source.contains("{{") {
        return Ok(source.to_owned());
    }
    let mut template = Template::default();
    template.add_func("join", template_join);
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

fn template_join(arguments: &[TemplateValue]) -> Result<TemplateValue, FuncError> {
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

fn template_regex_replace(arguments: &[TemplateValue]) -> Result<TemplateValue, FuncError> {
    if arguments.len() != 3 {
        return Err(FuncError::ExactlyXArgs("re_replace".to_owned(), 3));
    }
    let mut builder = FancyRegexBuilder::new(&arguments[1].to_string());
    builder.backtrack_limit(CARDIGANN_REGEX_BACKTRACK_LIMIT);
    let expression = builder
        .build()
        .map_err(|error| FuncError::Generic(error.to_string()))?;
    let input = arguments[0].to_string();
    let rendered = expression
        .try_replacen(&input, 0, arguments[2].to_string())
        .map_err(|error| FuncError::Generic(error.to_string()))?;
    Ok(TemplateValue::String(rendered.into_owned()))
}

fn resolve_category_ids(definition: &IndexerDefinition, requested: Option<&str>) -> Vec<String> {
    let Some(requested) = requested else {
        return Vec::new();
    };
    let Some(caps) = mapping(&definition.spec.implementation.capabilities) else {
        return Vec::new();
    };
    if let Some(mappings) = caps
        .get(key("categorymappings"))
        .and_then(YamlValue::as_sequence)
    {
        let ids = mappings
            .iter()
            .filter_map(mapping)
            .filter(|item| {
                item.get(key("cat"))
                    .and_then(scalar)
                    .is_some_and(|category| {
                        category == requested || category.starts_with(&format!("{requested}/"))
                    })
            })
            .filter_map(|item| item.get(key("id")).and_then(scalar))
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            return ids;
        }
    }
    let Some(categories) = caps.get(key("categories")).and_then(mapping) else {
        return Vec::new();
    };
    categories
        .iter()
        .filter(|(name, _)| scalar(name).as_deref() == Some(requested))
        .flat_map(|(_, ids)| ids.as_sequence().cloned().unwrap_or_default())
        .filter_map(|value| scalar(&value))
        .collect()
}

fn all_search_paths(search: &Mapping) -> Result<Vec<&Mapping>, EngineError> {
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

fn retryable_search_error(error: &EngineError) -> bool {
    match error {
        EngineError::HttpClient(error) => error.is_timeout() || error.is_connect(),
        EngineError::UnexpectedStatus(status) => (500..=599).contains(status),
        _ => false,
    }
}

fn render_inputs(
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

fn render_search_inputs(
    search: &Mapping,
    path: &Mapping,
    context: &TemplateValue,
    allow_empty: bool,
) -> Result<Vec<(String, String)>, EngineError> {
    let mut inputs = render_inputs(path.get(key("inputs")), context, allow_empty)?;
    inputs.extend(render_inputs(
        search.get(key("inputs")),
        context,
        allow_empty,
    )?);
    Ok(inputs)
}

fn render_headers(
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

fn append_query(url: &mut Url, inputs: &[(String, String)], raw_query: Option<&str>) {
    {
        let mut query = url.query_pairs_mut();
        query.extend_pairs(
            inputs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    if let Some(raw) = raw_query {
        let raw = raw.trim_start_matches(['?', '&']);
        let mut query = url.query_pairs_mut();
        query.extend_pairs(url::form_urlencoded::parse(raw.as_bytes()));
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "cardigann/tests.rs"]
mod tests;
