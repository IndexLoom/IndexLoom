//! Dedicated origin-confined transport. Never forward redirects or expose remote text.
use super::{ApiError, Integration, IntegrationSecrets};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::Value;
use std::time::Duration;
const LIMIT: usize = 1024 * 1024;
pub(super) struct Remote {
    pub client: Client,
    base: url::Url,
    pub secrets: IntegrationSecrets,
}
impl Remote {
    pub async fn new(item: &Integration, secrets: IntegrationSecrets) -> Result<Self, ApiError> {
        let base = super::origin(&item.origin)?;
        let host = base
            .host_str()
            .ok_or_else(failure)?
            .trim_matches(['[', ']']);
        let port = base.port_or_known_default().ok_or_else(failure)?;
        let addresses: Vec<_> = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?
        .collect();
        // HTTP to a single-label or private-LAN hostname must never be
        // permitted to resolve to a public host: API keys are sent in the clear.
        let http_lan_only = base.scheme() == "http"
            && !host.eq_ignore_ascii_case("localhost")
            && !host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if addresses.is_empty()
            || addresses.iter().any(|a| {
                !allowed_ip(a.ip(), host, super::allow_lan())
                    || (http_lan_only && !is_lan_ip(a.ip()))
            })
        {
            return Err(ApiError::bad_request(
                "integration DNS resolved to a forbidden address",
            ));
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| failure())?;
        Ok(Self {
            client,
            base,
            secrets,
        })
    }
    pub fn secret(&self, key: &str) -> Result<&str, ApiError> {
        self.secrets
            .0
            .get(key)
            .filter(|v| !v.is_empty())
            .map(String::as_str)
            .ok_or_else(|| ApiError::bad_request("required integration credential is missing"))
    }
    pub fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, ApiError> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['?', '#', '\\']) {
            return Err(ApiError::bad_request("invalid adapter path"));
        }
        let url = self.base.join(path).map_err(|_| failure())?;
        if url.origin() != self.base.origin() {
            return Err(ApiError::bad_request("adapter origin mismatch"));
        }
        Ok(self.client.request(method, url))
    }
    pub async fn send(&self, request: RequestBuilder) -> Result<Value, ApiError> {
        let response = request.send().await.map_err(|_| failure())?;
        self.response(response).await
    }
    pub async fn response(&self, mut response: reqwest::Response) -> Result<Value, ApiError> {
        if !response.status().is_success() {
            return Err(failure());
        }
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err(failure());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
            if bytes.len() + chunk.len() > LIMIT {
                return Err(failure());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|_| failure())
    }
    pub async fn arr(
        &self,
        version: &str,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        let mut req = self
            .request(method, &format!("/api/{version}/{path}"))?
            .header("X-Api-Key", self.secret("apiKey")?);
        if let Some(body) = body {
            req = req.json(body);
        }
        self.send(req).await
    }
}
pub(super) fn failure() -> ApiError {
    ApiError::bad_gateway("integration request failed or returned an invalid response")
}

fn is_lan_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let a = v.octets();
            a[0] == 10 || (a[0] == 172 && (16..=31).contains(&a[1])) || a[0..2] == [192, 168]
        }
        std::net::IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn allowed_ip(ip: std::net::IpAddr, host: &str, allow_lan: bool) -> bool {
    let explicit_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|v| v.is_loopback());
    if ip.is_loopback() {
        return explicit_loopback;
    }
    // Cloud metadata endpoints remain forbidden even under explicit LAN opt-in.
    if ip
        == "fd00:ec2::254"
            .parse::<std::net::IpAddr>()
            .expect("fixed IP")
        || ip
            == "168.63.129.16"
                .parse::<std::net::IpAddr>()
                .expect("fixed IP")
    {
        return false;
    }
    let trusted_name = host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
        || host.rsplit_once('.').is_some_and(|(_, suffix)| {
            suffix.eq_ignore_ascii_case("local") || suffix.eq_ignore_ascii_case("internal")
        });
    if allow_lan && trusted_name && is_lan_ip(ip) {
        return true;
    }
    match ip {
        std::net::IpAddr::V4(v) => {
            !(v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_multicast()
                || v.is_broadcast()
                || v.is_documentation()
                || (v.octets()[0..3] == [192, 0, 0])
                || (v.octets()[0..3] == [192, 88, 99])
                || v.octets()[0] == 0
                || v.octets()[0] >= 224
                || v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1])
                || v.octets()[0] == 198 && (18..=19).contains(&v.octets()[1]))
        }
        std::net::IpAddr::V6(v) => {
            v.to_ipv4_mapped().is_none()
                && (v.segments()[0] & 0xe000) == 0x2000
                && (v.segments()[0..2] != [0x2001, 0x0db8])
                && v.segments()[0] != 0x2002
                && !(v.segments()[0] == 0x2001 && v.segments()[1] < 0x200)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_private_and_rebound_dns() {
        for ip in [
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "127.0.0.1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "::1",
        ] {
            assert!(!allowed_ip(ip.parse().unwrap(), "public.example", false));
        }
        assert!(allowed_ip("127.0.0.1".parse().unwrap(), "localhost", false));
        assert!(allowed_ip(
            "8.8.8.8".parse().unwrap(),
            "public.example",
            false
        ));
        for ip in [
            "169.254.1.1",
            "fe80::1",
            "fd00:ec2::254",
            "168.63.129.16",
            "::ffff:10.0.0.1",
        ] {
            assert!(!allowed_ip(ip.parse().unwrap(), "sonarr", true));
        }
        assert!(!allowed_ip(
            "172.18.0.5".parse().unwrap(),
            "public.example",
            true
        ));
        assert!(allowed_ip("172.18.0.5".parse().unwrap(), "sonarr", true));
        assert!(allowed_ip("fd00::12".parse().unwrap(), "sonarr", true));
        assert!(!allowed_ip(
            "169.254.169.254".parse().unwrap(),
            "sonarr",
            true
        ));
        assert!(!allowed_ip(
            "127.0.0.1".parse().unwrap(),
            "public.example",
            true
        ));
    }
}
