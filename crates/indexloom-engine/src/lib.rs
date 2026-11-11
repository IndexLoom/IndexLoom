//! Sandboxed direct HTTP and `FlareSolverr` execution primitives.

use std::{
    collections::BTreeSet,
    error::Error as StdError,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use reqwest::{
    Client, Method, RequestBuilder,
    dns::{Addrs, Name, Resolve, Resolving},
    header,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

mod cardigann;

pub use cardigann::{
    CardigannExecutor, IndexerExecutionConfig, IndexerSearchBatch, IndexerTestOutcome,
    SearchParameters,
};

const MAX_FLARESOLVERR_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// DNS resolver used only by direct tracker connections.
///
/// Resolution and address classification happen inside reqwest's connection
/// path, so the exact addresses returned here are the addresses the connector
/// may use. This avoids a validation-to-use DNS lookup gap. A mixed answer is
/// rejected in full rather than silently discarding its private alternatives.
#[derive(Clone, Debug, Default)]
struct PublicDnsResolver;

impl Resolve for PublicDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let hostname = name.as_str().to_owned();
        Box::pin(async move {
            let resolved = tokio::net::lookup_host((hostname.as_str(), 0))
                .await
                .map_err(boxed_dns_error)?;
            let addresses = resolved.collect::<BTreeSet<_>>();
            if addresses.is_empty() {
                return Err(boxed_dns_error(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "DNS returned no addresses",
                )));
            }
            if addresses.iter().any(|address| !is_public_ip(address.ip())) {
                return Err(boxed_dns_error(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "DNS returned a non-public address",
                )));
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

fn boxed_dns_error(error: io::Error) -> Box<dyn StdError + Send + Sync> {
    Box::new(error)
}

/// Rejects URL IP literals that would bypass DNS resolution.
fn validate_public_url_literal(url: &Url) -> Result<(), EngineError> {
    let public = match url.host() {
        Some(url::Host::Ipv4(address)) => is_public_ipv4(address),
        Some(url::Host::Ipv6(address)) => is_public_ipv6(address),
        Some(url::Host::Domain(_)) => true,
        None => false,
    };
    if public {
        Ok(())
    } else {
        Err(EngineError::OutboundAddressRejected)
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    if address == Ipv4Addr::new(168, 63, 129, 16) || address == Ipv4Addr::new(147, 75, 207, 207) {
        return false;
    }
    !matches!(
        (a, b, c),
        (0 | 10 | 127 | 224..=255, _, _)
            | (100, 64..=127, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0 | 168, _)
            | (192, 88, 99)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();

    // Well-known NAT64 embeds an IPv4 destination in the low 32 bits.
    if segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
        return is_public_ipv4(embedded_ipv4(address));
    }
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return false;
    }

    // Fail closed on IANA-reserved global-unicast space. These ranges are the
    // currently allocated RIR blocks from the IPv6 Global Unicast registry;
    // 2001::/23 and deprecated 6to4 remain excluded as special-purpose space.
    match segments[0] {
        0x2001 => matches!(
            segments[1],
            0x0200..=0x0fff
                | 0x1200..=0x3fff
                | 0x4000..=0x4dff
                | 0x5000..=0x5fff
                | 0x8000..=0xbfff
        ),
        0x2003 => segments[1] <= 0x3fff,
        0x2400..=0x241f
        | 0x2600..=0x260f
        | 0x2630..=0x263f
        | 0x2800..=0x280f
        | 0x2a00..=0x2a1f
        | 0x2c00..=0x2c0f => true,
        0x2610 | 0x2620 => segments[1] <= 0x01ff,
        _ => false,
    }
}

fn embedded_ipv4(address: Ipv6Addr) -> Ipv4Addr {
    let octets = address.octets();
    Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15])
}

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
        Self::with_proxy(None)
    }

    /// Builds an isolated cookie store and optional HTTP/SOCKS route for one
    /// indexer. A client is never shared across tracker instances.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the proxy or HTTP client is invalid.
    pub fn with_proxy(proxy: Option<&ProxyRoute>) -> Result<Self, EngineError> {
        if proxy.is_some_and(|route| !route.enabled) {
            return Err(EngineError::ProxyDisabled);
        }
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .cookie_store(true)
            // Environment proxy variables must not silently replace the
            // explicit direct/proxy routing decision made by IndexLoom.
            .no_proxy();
        if let Some(route) = proxy {
            let mut configured = reqwest::Proxy::all(&route.url)?;
            if let Some(username) = route.username.as_deref() {
                configured =
                    configured.basic_auth(username, route.password.as_deref().unwrap_or(""));
            }
            builder = builder.proxy(configured);
        } else {
            // HTTP/SOCKS remote-DNS proxies and FlareSolverr are explicit
            // administrator-controlled egress boundaries. Only a direct
            // connection can be authoritatively constrained by local DNS.
            builder = builder.dns_resolver(PublicDnsResolver);
        }
        let client = builder.build()?;
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

/// Execution-only proxy route. The password is deliberately not serializable
/// and is redacted from debug output.
#[derive(Clone)]
pub struct ProxyRoute {
    pub id: String,
    pub url: String,
    pub enabled: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for ProxyRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyRoute")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("enabled", &self.enabled)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
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
            .no_proxy()
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
        proxy: Option<&ProxyRoute>,
    ) -> Result<BrowserSolution, EngineError> {
        let request = FlareSolverrRequest {
            command: "request.get",
            url: target,
            max_timeout_ms: u64::try_from(self.max_timeout.as_millis()).unwrap_or(u64::MAX),
            cookies,
            proxy: proxy.map(FlareSolverrProxy::from),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<FlareSolverrProxy>,
}

#[derive(Debug, Serialize)]
struct FlareSolverrProxy {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

impl From<&ProxyRoute> for FlareSolverrProxy {
    fn from(proxy: &ProxyRoute) -> Self {
        // FlareSolverr accepts the standard SOCKS scheme names while reqwest
        // uses the `a`/`h` suffixes to make remote DNS resolution explicit.
        let url = proxy
            .url
            .strip_prefix("socks4a://")
            .map(|rest| format!("socks4://{rest}"))
            .or_else(|| {
                proxy
                    .url
                    .strip_prefix("socks5h://")
                    .map(|rest| format!("socks5://{rest}"))
            })
            .unwrap_or_else(|| proxy.url.clone());
        Self {
            url,
            username: proxy.username.clone(),
            password: proxy.password.clone(),
        }
    }
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
    /// A definition or runtime endpoint is not a valid URL.
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
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
    /// A route is assigned but administratively disabled.
    #[error("assigned outbound proxy is disabled")]
    ProxyDisabled,
    /// The definition has no usable base URL.
    #[error("indexer definition has no base URL")]
    MissingBaseUrl,
    /// Search input was empty.
    #[error("search term must not be empty")]
    EmptySearchTerm,
    /// A dynamic Cardigann document is structurally invalid.
    #[error("invalid Cardigann search definition: {0}")]
    InvalidSearchDefinition(&'static str),
    /// A Go-compatible Cardigann template could not be rendered.
    #[error("Cardigann template failed: {0}")]
    Template(String),
    /// A filter expression was malformed.
    #[error("Cardigann filter failed: {0}")]
    InvalidFilter(String),
    /// An HTTP header name in a definition is invalid.
    #[error("invalid Cardigann header name: {0}")]
    InvalidHeaderName(String),
    /// The HTTP method is not supported by this execution stage.
    #[error("unsupported Cardigann HTTP method: {0}")]
    UnsupportedHttpMethod(String),
    /// The response format is not supported by this execution stage.
    #[error("unsupported Cardigann response type: {0}")]
    UnsupportedResponseType(String),
    /// Authentication requires a workflow not yet representable safely.
    #[error("unsupported Cardigann authentication: {0}")]
    UnsupportedAuthentication(String),
    /// Login form expected by the definition was absent.
    #[error("tracker authentication form was not found")]
    AuthenticationFormMissing,
    /// Login/test selectors indicate rejected credentials.
    #[error("tracker authentication was rejected")]
    AuthenticationRejected,
    /// `FlareSolverr` cannot represent the required request without losing headers.
    #[error("FlareSolverr route supports GET searches without custom headers only")]
    FlareSolverrUnsupportedRequest,
    /// A response used an invalid numeric HTTP status.
    #[error("invalid HTTP response status: {0}")]
    InvalidResponseStatus(u16),
    /// A direct response exceeded the bounded memory budget.
    #[error("tracker response exceeded the 16 MiB size limit")]
    ResponseTooLarge,
    /// A tracker returned an unsuccessful HTTP status.
    #[error("tracker returned HTTP {0}")]
    UnexpectedStatus(u16),
    /// A redirect did not carry a Location header.
    #[error("tracker redirect did not include a Location header")]
    RedirectWithoutLocation,
    /// Redirect policy budget was exceeded.
    #[error("tracker exceeded the maximum redirect count")]
    TooManyRedirects,
    /// A definition tried to leave its declared tracker origins.
    #[error("outbound URL was rejected by the definition origin policy: {0}")]
    OutboundUrlRejected(String),
    /// A direct request resolved to a non-public network address.
    #[error("outbound URL was rejected by the public network address policy")]
    OutboundAddressRejected,
}

#[cfg(test)]
mod tests {
    use reqwest::{Method, header};
    use url::Url;

    use std::{net::IpAddr, str::FromStr};

    use reqwest::dns::{Name, Resolve};

    use super::{
        DirectRequestProfile, FlareSolverrClient, PublicDnsResolver, SearchEngine, is_public_ip,
        validate_public_url_literal,
    };

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

    #[test]
    fn classifies_public_and_forbidden_network_addresses() {
        for address in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "64:ff9b::0808:0808",
        ] {
            assert!(
                is_public_ip(IpAddr::from_str(address).unwrap()),
                "{address} should be public"
            );
        }
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.100.100.200",
            "127.0.0.1",
            "169.254.169.254",
            "168.63.129.16",
            "147.75.207.207",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:0001",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2001:100::1",
            "2002:7f00:1::1",
            "2d00::1",
            "ff02::1",
        ] {
            assert!(
                !is_public_ip(IpAddr::from_str(address).unwrap()),
                "{address} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_forbidden_ip_literals_before_transport() {
        assert!(validate_public_url_literal(&Url::parse("http://127.0.0.1/").unwrap()).is_err());
        assert!(
            validate_public_url_literal(&Url::parse("http://[::ffff:127.0.0.1]/").unwrap())
                .is_err()
        );
        assert!(validate_public_url_literal(&Url::parse("https://1.1.1.1/").unwrap()).is_ok());
    }

    #[tokio::test]
    async fn resolver_rejects_localhost_answers() {
        let name = Name::from_str("localhost").expect("localhost should be a DNS name");
        let Err(error) = PublicDnsResolver.resolve(name).await else {
            panic!("localhost must not resolve through the public resolver");
        };
        assert!(error.to_string().contains("non-public"));
    }

    #[tokio::test]
    async fn direct_http_transport_cannot_connect_to_localhost() {
        let engine = SearchEngine::new().expect("engine should initialize");
        let error = engine
            .client()
            .get("http://localhost/")
            .send()
            .await
            .expect_err("the direct transport must reject localhost DNS answers");
        let mut messages = vec![error.to_string()];
        let mut source = std::error::Error::source(&error);
        while let Some(error) = source {
            messages.push(error.to_string());
            source = error.source();
        }
        assert!(
            messages
                .iter()
                .any(|message| message.contains("non-public")),
            "the resolver rejection must remain visible in the error chain: {messages:?}"
        );
    }
}
