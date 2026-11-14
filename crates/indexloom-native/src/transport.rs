//! The only network boundary. There is deliberately no built-in socket client.

use std::{collections::BTreeMap, fmt, time::Duration};

use async_trait::async_trait;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

/// A credential or sensitive header. Debug always redacts the value.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Only expose to the transport or credential validation code; never log.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// Transport input. URLs, headers and bodies can contain credentials.
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: BTreeMap<String, Secret>,
    pub body: Zeroizing<Vec<u8>>,
    /// Unique per adapter object, even when configured instance IDs match.
    pub session_id: Uuid,
    pub timeout: Duration,
    /// Must be enforced while streaming, after decompression, before allocation.
    pub max_response_bytes: usize,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &"[REDACTED]")
            .field("session_id", &self.session_id)
            .field("timeout", &self.timeout)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish_non_exhaustive()
    }
}

/// A single response. Header names are matched case-insensitively.
pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// Closed errors prevent HTTP library diagnostics from leaking signed URLs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("egress policy denied the request")]
    PolicyDenied,
    #[error("transient network failure")]
    Transient,
    #[error("network request timed out")]
    Timeout,
    #[error("response exceeded the streaming byte limit")]
    BodyTooLarge,
    #[error("non-retryable transport failure")]
    Permanent,
}

/// Caller-supplied egress enforcement, used for **every** attempt and download.
///
/// Implementations MUST validate the destination with the host's SSRF policy,
/// resolve and pin permitted IPs to the actual connection (including proxy
/// routing), and reject private/reserved/link-local/metadata targets unless
/// explicitly permitted by host policy. A hostname-only check is insufficient.
/// Redirects MUST be disabled: return the original 3xx without following it.
/// Enforce `timeout` and the decompressed streaming `max_response_bytes` bound.
/// Disable ambient cookies/auth; any cookie state MUST be scoped to `session_id`.
/// Never log URL queries, headers, bodies or raw errors. Do not retry internally;
/// retry accounting and delays belong to the adapter. Cancellation must stop I/O.
///
/// This trait is a trust boundary, not an assertion that arbitrary implementors
/// are safe. The host must supply its validated transport before enabling A12.
#[async_trait]
pub trait ValidatedTransport: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;
}
