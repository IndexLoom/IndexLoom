//! Sandboxed direct HTTP and `FlareSolverr` execution primitives.

use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, header};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

const MAX_FLARESOLVERR_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Shared HTTP execution engine.
#[derive(Clone, Debug)]
pub struct SearchEngine {
    client: Client,
}

impl SearchEngine {
    /// Builds a client with redirects disabled until definition policy is applied.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the TLS or HTTP client cannot be initialized.
    pub fn new() -> Result<Self, EngineError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { client })
    }

    /// Exposes the configured client to bounded request executors.
    pub const fn client(&self) -> &Client {
        &self.client
    }

    /// Builds a direct request with credentials scoped to this request only.
    ///
    /// A shared cookie jar is intentionally not used: credentials from one
    /// tracker must never be replayed to another tracker.
    pub fn request(
        &self,
        method: Method,
        url: Url,
        profile: &DirectRequestProfile,
    ) -> RequestBuilder {
        let mut request = self.client.request(method, url);
        if let Some(user_agent) = profile.user_agent.as_deref() {
            request = request.header(header::USER_AGENT, user_agent);
        }
        if let Some(cookie) = profile.cookie_header.as_deref() {
            request = request.header(header::COOKIE, cookie);
        }
        request
    }

    /// Extracts text values from an HTML document for conformance tests.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when `selector` is not valid CSS.
    pub fn extract_text(html: &str, selector: &str) -> Result<Vec<String>, EngineError> {
        let selector = Selector::parse(selector)
            .map_err(|error| EngineError::InvalidSelector(error.to_string()))?;
        let document = Html::parse_document(html);
        Ok(document
            .select(&selector)
            .map(|element| element.text().collect::<String>().trim().to_owned())
            .collect())
    }
}

/// Per-indexer direct HTTP values. The type deliberately does not implement
/// [`Serialize`] so cookies cannot be returned from an API accidentally.
pub struct DirectRequestProfile {
    /// Optional browser `User-Agent` override.
    pub user_agent: Option<String>,
    /// Optional raw Cookie request header.
    pub cookie_header: Option<String>,
}

impl std::fmt::Debug for DirectRequestProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectRequestProfile")
            .field("user_agent", &self.user_agent)
            .field(
                "cookie_header",
                &self.cookie_header.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Minimal `FlareSolverr` API client with bounded responses.
#[derive(Clone, Debug)]
pub struct FlareSolverrClient {
    client: Client,
    endpoint: Url,
    max_timeout: Duration,
}

impl FlareSolverrClient {
    /// Creates a connector for an already validated `/v1` endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the HTTP client cannot be built or the URL is
    /// not an HTTP(S) `/v1` endpoint.
    pub fn new(endpoint: Url, max_timeout: Duration) -> Result<Self, EngineError> {
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.path() != "/v1" {
            return Err(EngineError::InvalidFlareSolverrEndpoint);
        }
        if !(Duration::from_secs(5)..=Duration::from_secs(300)).contains(&max_timeout) {
            return Err(EngineError::InvalidFlareSolverrTimeout);
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(max_timeout + Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            max_timeout,
        })
    }

    /// Requests a browser-solved page.
    ///
    /// The caller must apply the engine's outbound URL policy before passing a
    /// target URL. Cookies are transported only in the authenticated JSON body
    /// sent to the configured connector.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for transport errors, oversized or malformed
    /// responses, and solver failures.
    pub async fn get(
        &self,
        target: &Url,
        cookies: &[BrowserCookie],
    ) -> Result<BrowserSolution, EngineError> {
        let request = FlareSolverrRequest {
            command: "request.get",
            url: target,
            max_timeout_ms: u64::try_from(self.max_timeout.as_millis()).unwrap_or(u64::MAX),
            cookies,
        };
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&request)
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FLARESOLVERR_RESPONSE_BYTES as u64)
        {
            return Err(EngineError::FlareSolverrResponseTooLarge);
        }
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_FLARESOLVERR_RESPONSE_BYTES {
            return Err(EngineError::FlareSolverrResponseTooLarge);
        }
        let envelope: FlareSolverrResponse = serde_json::from_slice(&bytes)?;
        if envelope.status != "ok" {
            return Err(EngineError::FlareSolverrRejected(
                envelope
                    .message
                    .unwrap_or_else(|| "solver returned an error".to_owned()),
            ));
        }
        envelope
            .solution
            .ok_or(EngineError::MissingFlareSolverrSolution)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlareSolverrRequest<'a> {
    #[serde(rename = "cmd")]
    command: &'static str,
    url: &'a Url,
    #[serde(rename = "maxTimeout")]
    max_timeout_ms: u64,
    #[serde(skip_serializing_if = "<[BrowserCookie]>::is_empty")]
    cookies: &'a [BrowserCookie],
}

#[derive(Debug, Deserialize)]
struct FlareSolverrResponse {
    status: String,
    message: Option<String>,
    solution: Option<BrowserSolution>,
}

/// Browser cookie accepted and returned by `FlareSolverr`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value. Callers must redact this field from logs and APIs.
    pub value: String,
    /// Optional cookie domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Optional cookie path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Secure-cookie flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub secure: bool,
    /// HTTP-only flag.
    #[serde(default, rename = "httpOnly", skip_serializing_if = "is_false")]
    pub http_only: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Browser response returned by a successful solver call.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSolution {
    /// HTTP status returned by the target.
    pub status: u16,
    /// Browser `User-Agent` that must be reused with returned cookies.
    pub user_agent: String,
    /// Solved response body.
    pub response: String,
    /// Updated browser cookies.
    #[serde(default)]
    pub cookies: Vec<BrowserCookie>,
}

/// Engine construction and parsing errors.
#[derive(Debug, Error)]
pub enum EngineError {
    /// HTTP client construction failed.
    #[error("failed to build HTTP client: {0}")]
    HttpClient(#[from] reqwest::Error),
    /// A solver response was not valid JSON.
    #[error("invalid FlareSolverr response: {0}")]
    FlareSolverrJson(#[from] serde_json::Error),
    /// A definition contains an invalid CSS selector.
    #[error("invalid CSS selector: {0}")]
    InvalidSelector(String),
    /// The connector URL is not an HTTP(S) `/v1` endpoint.
    #[error("invalid FlareSolverr endpoint")]
    InvalidFlareSolverrEndpoint,
    /// Solver timeout is outside the supported safety range.
    #[error("FlareSolverr timeout must be between 5 and 300 seconds")]
    InvalidFlareSolverrTimeout,
    /// The connector returned more data than the configured safety budget.
    #[error("FlareSolverr response exceeded the size limit")]
    FlareSolverrResponseTooLarge,
    /// The connector returned an application-level error.
    #[error("FlareSolverr rejected the request: {0}")]
    FlareSolverrRejected(String),
    /// A nominally successful response did not contain a solution.
    #[error("FlareSolverr response did not contain a solution")]
    MissingFlareSolverrSolution,
}

#[cfg(test)]
mod tests {
    use reqwest::{Method, header};
    use url::Url;

    use super::{DirectRequestProfile, FlareSolverrClient, SearchEngine};

    #[test]
    fn extracts_text_from_matching_nodes() {
        let values = SearchEngine::extract_text(
            r#"<div class="result">First</div><div class="result">Second</div>"#,
            ".result",
        )
        .expect("selector should be valid");

        assert_eq!(values, ["First", "Second"]);
    }

    #[test]
    fn scopes_user_agent_and_cookie_to_one_request() {
        let engine = SearchEngine::new().expect("engine should initialize");
        let profile = DirectRequestProfile {
            user_agent: Some("IndexLoom test agent".to_owned()),
            cookie_header: Some("session=secret".to_owned()),
        };
        let request = engine
            .request(
                Method::GET,
                Url::parse("https://example.invalid/").expect("URL should parse"),
                &profile,
            )
            .build()
            .expect("request should build");
        assert_eq!(
            request.headers()[header::USER_AGENT],
            "IndexLoom test agent"
        );
        assert_eq!(request.headers()[header::COOKIE], "session=secret");

        let uncredentialed = engine
            .request(
                Method::GET,
                Url::parse("https://example.invalid/other").expect("URL should parse"),
                &DirectRequestProfile {
                    user_agent: None,
                    cookie_header: None,
                },
            )
            .build()
            .expect("request should build");
        assert!(!uncredentialed.headers().contains_key(header::COOKIE));
    }

    #[test]
    fn requires_flaresolverr_v1_endpoint() {
        let endpoint = Url::parse("http://flaresolverr:8191/v1").expect("URL should parse");
        assert!(FlareSolverrClient::new(endpoint, std::time::Duration::from_secs(60)).is_ok());
        let invalid = Url::parse("http://flaresolverr:8191/").expect("URL should parse");
        assert!(FlareSolverrClient::new(invalid, std::time::Duration::from_secs(60)).is_err());
    }
}
