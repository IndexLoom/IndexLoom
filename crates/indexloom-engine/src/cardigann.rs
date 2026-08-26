use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
    time::{Duration, Instant},
};

use gtmpl::{Context, FuncError, Template, Value as TemplateValue};
use indexloom_core::SearchResult;
use indexloom_definitions::{ConfigurationValue, IndexerDefinition};
use regex::Regex;
use reqwest::{Method, StatusCode, header::HeaderName};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
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
static HYPHENATED_CONFIG_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.Config\.([A-Za-z0-9_]+(?:-[A-Za-z0-9_]+)+)")
        .expect("static Cardigann config-reference regex must compile")
});

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
        let render = self.context.for_search(&keywords, BTreeMap::new());
        let path = first_search_path(search)?;
        let response = self.execute_search_path(search, path, &render).await?;
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
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_DIRECT_RESPONSE_BYTES {
            return Err(EngineError::ResponseTooLarge);
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
            ("Categories".to_owned(), TemplateValue::Array(Vec::new())),
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
    let expression = Regex::new(&arguments[1].to_string())
        .map_err(|error| FuncError::Generic(error.to_string()))?;
    Ok(TemplateValue::String(
        expression
            .replace_all(&arguments[0].to_string(), arguments[2].to_string())
            .into_owned(),
    ))
}

fn first_search_path(search: &Mapping) -> Result<&Mapping, EngineError> {
    if let Some(paths) = search.get(key("paths")).and_then(YamlValue::as_sequence) {
        return paths
            .first()
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search paths are empty",
            ));
    }
    if search.contains_key(key("path")) {
        return Ok(search);
    }
    Err(EngineError::InvalidSearchDefinition(
        "search path is missing",
    ))
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

fn parse_html_results(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let rows =
        search
            .get(key("rows"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search rows are missing",
            ))?;
    let selector =
        rows.get(key("selector"))
            .and_then(scalar)
            .ok_or(EngineError::InvalidSearchDefinition(
                "row selector is missing",
            ))?;
    let selector = render_template(&selector, &context.for_search(keywords, BTreeMap::new()))?;
    let (css, contains) = normalize_contains_selector(&selector);
    let selector =
        Selector::parse(&css).map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
    let document = Html::parse_document(body);
    document
        .select(&selector)
        .filter(|row| {
            contains
                .iter()
                .all(|needle| element_text(*row).contains(needle))
        })
        .filter_map(|row| {
            match extract_html_row(
                indexer_instance_id,
                definition,
                search,
                row,
                context,
                keywords,
                final_url,
            ) {
                Ok(Some(result)) => Some(Ok(result)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

fn extract_html_row(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    row: ElementRef<'_>,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Option<SearchResult>, EngineError> {
    let fields =
        search
            .get(key("fields"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search fields are missing",
            ))?;
    let mut result = BTreeMap::new();
    for (name, specification) in fields {
        let Some(name) = scalar(name) else { continue };
        let Some(specification) = mapping(specification) else {
            continue;
        };
        if specification.contains_key(key("text")) {
            continue;
        }
        if let Some(value) = extract_html_field(row, specification, context, keywords, &result)? {
            result.insert(name, value);
        }
    }
    for _ in 0..3 {
        let before = result.len();
        for (name, specification) in fields {
            let Some(name) = scalar(name) else { continue };
            if result.contains_key(&name) {
                continue;
            }
            let Some(specification) = mapping(specification) else {
                continue;
            };
            if let Some(value) = extract_html_field(row, specification, context, keywords, &result)?
            {
                result.insert(name, value);
            }
        }
        if result.len() == before {
            break;
        }
    }
    Ok(normalized_result(
        indexer_instance_id,
        definition,
        &result,
        final_url,
    ))
}

fn extract_html_field(
    row: ElementRef<'_>,
    specification: &Mapping,
    context: &RenderContext,
    keywords: &str,
    result: &BTreeMap<String, String>,
) -> Result<Option<String>, EngineError> {
    let render = context.for_search(keywords, result.clone());
    let mut value = if let Some(text) = specification.get(key("text")).and_then(scalar) {
        render_template(&text, &render)?
    } else if let Some(selector_text) = specification.get(key("selector")).and_then(scalar) {
        let selector_text = render_template(&selector_text, &render)?;
        let (css, contains) = normalize_contains_selector(&selector_text);
        let selector = Selector::parse(&css)
            .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
        let element = row.select(&selector).find(|element| {
            contains
                .iter()
                .all(|needle| element_text(*element).contains(needle))
        });
        let Some(element) = element else {
            return Ok(specification.get(key("default")).and_then(scalar));
        };
        if let Some(attribute) = specification.get(key("attribute")).and_then(scalar) {
            element
                .value()
                .attr(&attribute)
                .unwrap_or_default()
                .to_owned()
        } else {
            element_text(element)
        }
    } else {
        specification
            .get(key("default"))
            .and_then(scalar)
            .unwrap_or_default()
    };
    if let Some(cases) = specification.get(key("case")).and_then(mapping) {
        value = apply_case(&value, cases);
    }
    if let Some(filters) = specification.get(key("filters")) {
        value = apply_filters(value, filters)?;
    }
    if value.trim().is_empty()
        && specification
            .get(key("optional"))
            .and_then(YamlValue::as_bool)
            .unwrap_or(false)
    {
        Ok(None)
    } else {
        Ok(Some(value.trim().to_owned()))
    }
}

fn parse_json_results(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    body: &str,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Vec<SearchResult>, EngineError> {
    let document: JsonValue = serde_json::from_str(body)?;
    let rows =
        search
            .get(key("rows"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search rows are missing",
            ))?;
    let selector = rows
        .get(key("selector"))
        .and_then(scalar)
        .unwrap_or_default();
    let selected = json_path(&document, &selector).unwrap_or(&document);
    let rows = selected
        .as_array()
        .map_or_else(|| std::slice::from_ref(selected), Vec::as_slice);
    let attribute = rows_mapping_attribute(search);
    rows.iter()
        .filter_map(|row| {
            let row = attribute
                .as_deref()
                .and_then(|path| json_path(row, path))
                .unwrap_or(row);
            match extract_json_row(
                indexer_instance_id,
                definition,
                search,
                row,
                context,
                keywords,
                final_url,
            ) {
                Ok(Some(result)) => Some(Ok(result)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

fn rows_mapping_attribute(search: &Mapping) -> Option<String> {
    search
        .get(key("rows"))
        .and_then(mapping)
        .and_then(|rows| rows.get(key("attribute")))
        .and_then(scalar)
}

fn extract_json_row(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    search: &Mapping,
    row: &JsonValue,
    context: &RenderContext,
    keywords: &str,
    final_url: &Url,
) -> Result<Option<SearchResult>, EngineError> {
    let fields =
        search
            .get(key("fields"))
            .and_then(mapping)
            .ok_or(EngineError::InvalidSearchDefinition(
                "search fields are missing",
            ))?;
    let mut result = BTreeMap::new();
    for _ in 0..3 {
        let before = result.len();
        for (name, specification) in fields {
            let Some(name) = scalar(name) else { continue };
            if result.contains_key(&name) {
                continue;
            }
            let Some(specification) = mapping(specification) else {
                continue;
            };
            let render = context.for_search(keywords, result.clone());
            let mut value = if let Some(text) = specification.get(key("text")).and_then(scalar) {
                render_template(&text, &render)?
            } else if let Some(selector) = specification.get(key("selector")).and_then(scalar) {
                json_path(row, &selector)
                    .map(json_string)
                    .unwrap_or_default()
            } else {
                specification
                    .get(key("default"))
                    .and_then(scalar)
                    .unwrap_or_default()
            };
            if let Some(cases) = specification.get(key("case")).and_then(mapping) {
                value = apply_case(&value, cases);
            }
            if let Some(filters) = specification.get(key("filters")) {
                value = apply_filters(value, filters)?;
            }
            if !value.trim().is_empty() {
                result.insert(name, value.trim().to_owned());
            }
        }
        if result.len() == before {
            break;
        }
    }
    Ok(normalized_result(
        indexer_instance_id,
        definition,
        &result,
        final_url,
    ))
}

fn normalized_result(
    indexer_instance_id: &str,
    definition: &IndexerDefinition,
    values: &BTreeMap<String, String>,
    base: &Url,
) -> Option<SearchResult> {
    let title = values
        .get("title")
        .filter(|value| !value.is_empty())
        .cloned()?;
    let details_url =
        result_value(values, &["details"]).and_then(|value| normalize_http_link(base, value));
    let explicit_magnet = result_value(values, &["magnet"]).and_then(normalize_magnet);
    let raw_download = result_value(values, &["download"]);
    let magnet_url = explicit_magnet.or_else(|| raw_download.and_then(normalize_magnet));
    let download_url = raw_download.and_then(|value| normalize_http_link(base, value));
    if details_url.is_none() && download_url.is_none() && magnet_url.is_none() {
        return None;
    }
    let info_hash = result_value(values, &["infohash", "info_hash"])
        .and_then(normalize_info_hash)
        .or_else(|| magnet_url.as_deref().and_then(info_hash_from_magnet));
    Some(SearchResult {
        id: Uuid::now_v7(),
        indexer_instance_id: indexer_instance_id.to_owned(),
        definition_id: definition.metadata.id.clone(),
        source_id: result_value(
            values,
            &[
                "sourceid",
                "source_id",
                "torrentid",
                "torrent_id",
                "id",
                "_id",
            ],
        )
        .map(ToOwned::to_owned),
        source_guid: result_value(values, &["sourceguid", "source_guid", "guid"])
            .map(ToOwned::to_owned),
        info_hash,
        title,
        details_url,
        download_url,
        magnet_url,
        category: values.get("category").cloned(),
        size: values.get("size").and_then(|value| parse_size(value)),
        seeders: values.get("seeders").and_then(|value| parse_u32(value)),
        leechers: values.get("leechers").and_then(|value| parse_u32(value)),
        published_at: values.get("date").and_then(|value| parse_date(value)),
    })
}

fn result_value<'a>(values: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| values.get(*key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalize_magnet(value: &str) -> Option<String> {
    let value = value.trim();
    let url = Url::parse(value).ok()?;
    if !url.scheme().eq_ignore_ascii_case("magnet") {
        return None;
    }
    let has_exact_topic = url.query_pairs().any(|(name, value)| {
        name.eq_ignore_ascii_case("xt")
            && (value
                .get(..9)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
                || value
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btmh:")))
    });
    has_exact_topic.then(|| value.to_owned())
}

fn info_hash_from_magnet(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    url.query_pairs()
        .find(|(name, value)| {
            name.eq_ignore_ascii_case("xt")
                && value
                    .get(..9)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
        })
        .and_then(|(_, value)| normalize_info_hash(&value[9..]))
}

fn normalize_info_hash(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .get(..9)
        .filter(|prefix| prefix.eq_ignore_ascii_case("urn:btih:"))
        .map_or(value, |_| &value[9..]);
    if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(value.to_ascii_lowercase());
    }
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'2'..=b'7'))
    {
        return Some(value.to_ascii_uppercase());
    }
    None
}

fn normalize_http_link(base: &Url, value: &str) -> Option<String> {
    let url = base.join(value).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn json_path<'a>(mut value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    if path.is_empty() || path == ":root" {
        return Some(value);
    }
    for segment in path.trim_start_matches("$.").split('.') {
        if segment.is_empty() {
            continue;
        }
        let mut name = segment;
        let mut index = None;
        if let Some(open) = segment.find('[') {
            name = &segment[..open];
            index = segment[open + 1..]
                .trim_end_matches(']')
                .parse::<usize>()
                .ok();
        }
        if !name.is_empty() {
            value = value.get(name)?;
        }
        if let Some(index) = index {
            value = value.get(index)?;
        }
    }
    Some(value)
}

fn json_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => String::new(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(values) => values.iter().map(json_string).collect::<Vec<_>>().join(" "),
        JsonValue::Object(_) => value.to_string(),
    }
}

fn apply_case(value: &str, cases: &Mapping) -> String {
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

fn apply_filters(mut value: String, filters: &YamlValue) -> Result<String, EngineError> {
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
                let regex = Regex::new(&args[0])
                    .map_err(|error| EngineError::InvalidFilter(error.to_string()))?;
                value = regex.replace_all(&value, args[1].as_str()).into_owned();
            }
            "regexp" if !args.is_empty() => {
                let regex = Regex::new(&args[0])
                    .map_err(|error| EngineError::InvalidFilter(error.to_string()))?;
                value = regex
                    .captures(&value)
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

fn filter_args(value: Option<&YamlValue>) -> Vec<String> {
    match value {
        Some(YamlValue::Sequence(values)) => values.iter().filter_map(scalar).collect(),
        Some(value) => scalar(value).into_iter().collect(),
        None => Vec::new(),
    }
}

fn normalize_contains_selector(value: &str) -> (String, Vec<String>) {
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

fn check_error_selectors(value: Option<&YamlValue>, body: &str) -> Result<(), EngineError> {
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

fn selector_matches(body: &str, selector: &str) -> Result<bool, EngineError> {
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

fn element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn browser_cookies(header: Option<&str>) -> Vec<BrowserCookie> {
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

fn mapping(value: &YamlValue) -> Option<&Mapping> {
    value.as_mapping()
}
fn key(value: &str) -> YamlValue {
    YamlValue::String(value.to_owned())
}

fn scalar(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Bool(value) => Some(value.to_string()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Null => Some(String::new()),
        _ => None,
    }
}

fn configuration_json(value: ConfigurationValue) -> JsonValue {
    match value {
        ConfigurationValue::String(value) => JsonValue::String(value),
        ConfigurationValue::Boolean(value) => JsonValue::Bool(value),
        ConfigurationValue::Integer(value) => JsonValue::from(value),
        ConfigurationValue::Number(value) => JsonValue::from(value),
    }
}

fn json_template_value(value: JsonValue) -> TemplateValue {
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

fn parse_u32(value: &str) -> Option<u32> {
    value.replace([',', ' '], "").parse().ok()
}

fn parse_size(value: &str) -> Option<u64> {
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

fn parse_date(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gtmpl::Value as TemplateValue;
    use indexloom_definitions::IndexerDefinition;
    use serde_yaml::Value as YamlValue;
    use url::Url;

    use super::{
        RenderContext, apply_filters, info_hash_from_magnet, normalize_contains_selector,
        normalized_result, render_search_inputs, render_template,
    };

    #[test]
    fn exposes_keywords_in_root_and_cardigann_query_contexts() {
        let context = RenderContext::new(BTreeMap::new(), BTreeMap::new())
            .for_search("one piece", BTreeMap::new());
        let rendered = render_template("{{ .Keywords }}|{{ .Query.Keywords }}", &context)
            .expect("both Cardigann keyword forms should render");
        assert_eq!(rendered, "one piece|one piece");
    }

    #[test]
    fn renders_cardigann_conditionals() {
        let context = TemplateValue::Object(std::collections::HashMap::from([
            (
                "Keywords".to_owned(),
                TemplateValue::String("movie".to_owned()),
            ),
            ("True".to_owned(), TemplateValue::Bool(true)),
            (
                "Config".to_owned(),
                TemplateValue::Object(std::collections::HashMap::from([(
                    "enabled".to_owned(),
                    TemplateValue::Bool(true),
                )])),
            ),
            (
                "Result".to_owned(),
                TemplateValue::Object(BTreeMap::new().into_iter().collect()),
            ),
        ]));
        let rendered = render_template(
            "{{ if and (.Keywords) (eq .Config.enabled .True) }}search/{{ .Keywords }}{{ end }}",
            &context,
        )
        .expect("template should render");
        assert_eq!(rendered, "search/movie");
    }

    #[test]
    fn renders_hyphenated_cardigann_configuration_names() {
        let context = TemplateValue::Object(std::collections::HashMap::from([(
            "Config".to_owned(),
            TemplateValue::Object(std::collections::HashMap::from([(
                "filter-id".to_owned(),
                TemplateValue::String("trusted".to_owned()),
            )])),
        )]));
        let rendered = render_template("{{ .Config.filter-id }}", &context)
            .expect("hyphenated config names should use the map index function");
        assert_eq!(rendered, "trusted");
    }

    #[test]
    fn combines_path_and_search_level_inputs() {
        let search: YamlValue = serde_yaml::from_str(
            "paths:\n  - path: /\n    inputs:\n      q: '{{ .Keywords }}'\ninputs:\n  f: 0\n",
        )
        .expect("search fixture should parse");
        let search = search.as_mapping().expect("search should be a mapping");
        let path = search["paths"]
            .as_sequence()
            .and_then(|paths| paths.first())
            .and_then(YamlValue::as_mapping)
            .expect("path should be a mapping");
        let context = TemplateValue::Object(std::collections::HashMap::from([(
            "Keywords".to_owned(),
            TemplateValue::String("one piece".to_owned()),
        )]));
        let inputs = render_search_inputs(search, path, &context, false)
            .expect("both input scopes should render");
        assert_eq!(
            inputs,
            [
                ("q".to_owned(), "one piece".to_owned()),
                ("f".to_owned(), "0".to_owned())
            ]
        );
    }

    #[test]
    fn applies_common_filters() {
        let filters: YamlValue =
            serde_yaml::from_str("- name: replace\n  args: ['-', ' ']\n- name: trim\n")
                .expect("filters should parse");
        assert_eq!(
            apply_filters(" test-release ".to_owned(), &filters).unwrap(),
            "test release"
        );
    }

    #[test]
    fn separates_contains_from_css() {
        let (selector, needles) = normalize_contains_selector("a.item:contains(\"verified\")");
        assert_eq!(selector, "a.item");
        assert_eq!(needles, ["verified"]);
    }

    #[test]
    fn normalizes_instance_identity_and_release_links() {
        let values = BTreeMap::from([
            ("title".to_owned(), "Example release".to_owned()),
            ("details".to_owned(), "/torrent/42".to_owned()),
            ("download".to_owned(), "/download/42.torrent".to_owned()),
            ("id".to_owned(), "42".to_owned()),
            ("guid".to_owned(), "tracker-guid-42".to_owned()),
            (
                "infohash".to_owned(),
                "0123456789ABCDEF0123456789ABCDEF01234567".to_owned(),
            ),
        ]);
        let result = normalized_result(
            "example-through-proxy",
            &definition(),
            &values,
            &Url::parse("https://example.invalid/base/").unwrap(),
        )
        .expect("result should normalize");

        assert_eq!(result.indexer_instance_id, "example-through-proxy");
        assert_eq!(result.definition_id, "example");
        assert_eq!(result.source_id.as_deref(), Some("42"));
        assert_eq!(result.source_guid.as_deref(), Some("tracker-guid-42"));
        assert_eq!(
            result.info_hash.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            result.details_url.as_deref(),
            Some("https://example.invalid/torrent/42")
        );
        assert_eq!(
            result.download_url.as_deref(),
            Some("https://example.invalid/download/42.torrent")
        );
        assert!(result.magnet_url.is_none());
    }

    #[test]
    fn recognizes_a_magnet_in_the_download_field() {
        let magnet = "magnet:?xt=urn%3Abtih%3AABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let values = BTreeMap::from([
            ("title".to_owned(), "Magnet release".to_owned()),
            ("download".to_owned(), magnet.to_owned()),
        ]);
        let result = normalized_result(
            "example-direct",
            &definition(),
            &values,
            &Url::parse("https://example.invalid/").unwrap(),
        )
        .expect("result should normalize");

        assert!(result.download_url.is_none());
        assert_eq!(result.magnet_url.as_deref(), Some(magnet));
        assert_eq!(
            result.info_hash.as_deref(),
            Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
        );
        assert_eq!(
            info_hash_from_magnet(magnet).as_deref(),
            Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
        );
    }

    #[test]
    fn rejects_unsafe_links_and_uses_a_valid_magnet_fallback() {
        let fallback = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
        let values = BTreeMap::from([
            ("title".to_owned(), "Safe result".to_owned()),
            ("details".to_owned(), "javascript:alert(1)".to_owned()),
            (
                "magnet".to_owned(),
                "https://example.invalid/not-a-magnet".to_owned(),
            ),
            ("download".to_owned(), fallback.to_owned()),
        ]);
        let result = normalized_result(
            "example-direct",
            &definition(),
            &values,
            &Url::parse("https://example.invalid/").unwrap(),
        )
        .expect("valid fallback magnet should retain the result");
        assert!(result.details_url.is_none());
        assert!(result.download_url.is_none());
        assert_eq!(result.magnet_url.as_deref(), Some(fallback));

        let only_unsafe = BTreeMap::from([
            ("title".to_owned(), "Unsafe result".to_owned()),
            ("details".to_owned(), "data:text/html,unsafe".to_owned()),
            ("download".to_owned(), "file:///etc/passwd".to_owned()),
            (
                "magnet".to_owned(),
                "magnet:?dn=missing-exact-topic".to_owned(),
            ),
        ]);
        assert!(
            normalized_result(
                "example-direct",
                &definition(),
                &only_unsafe,
                &Url::parse("https://example.invalid/").unwrap(),
            )
            .is_none()
        );
    }

    fn definition() -> IndexerDefinition {
        serde_yaml::from_str(
            r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata:
  id: example
  name: Example
  description: Example definition
  language: en-US
provenance:
  generatedBy: indexloom-importer/v1
  source:
    project: jackett
    repository: https://github.com/Jackett/Jackett
    revision: "0000000000000000000000000000000000000000"
    path: src/Jackett.Common/Definitions/example.yml
    sourceId: example
    license: GPL-2.0-only
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000"
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111"
spec:
  protocol: torrent
  privacy: public
  baseUrls: [https://example.invalid/]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings: []
    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }
"#,
        )
        .expect("test definition should deserialize")
    }
}
