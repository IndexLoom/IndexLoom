//! Binary, bounded transport shared by search, login and grab workflows.
use super::{
    CardigannExecutor, DirectRequestProfile, Duration, EngineError, MAX_DIRECT_RESPONSE_BYTES,
    MAX_REDIRECTS, Method, Origin, RequestSpec, StatusCode, Url,
};

#[derive(Debug)]
pub(super) struct BinaryResponse {
    pub status: StatusCode,
    pub body: Vec<u8>,
    pub final_url: Url,
}

#[cfg(test)]
use reqwest::cookie::CookieStore;

#[cfg(test)]
type MockTransportFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<(StatusCode, reqwest::header::HeaderMap, Vec<u8>), EngineError>,
            > + Send
            + 'a,
    >,
>;

#[cfg(test)]
pub(super) trait MockTransport: std::fmt::Debug + Send + Sync {
    fn send(&self, spec: RequestSpec) -> MockTransportFuture<'_>;
}

impl CardigannExecutor {
    pub(super) async fn execute_binary(
        &self,
        spec: RequestSpec,
        magnet_redirect: bool,
    ) -> Result<BinaryResponse, EngineError> {
        tokio::time::timeout(
            Duration::from_secs(45),
            self.execute_binary_inner(spec, magnet_redirect),
        )
        .await
        .map_err(|_| EngineError::RequestTimeout)?
    }

    async fn execute_binary_inner(
        &self,
        mut spec: RequestSpec,
        magnet_redirect: bool,
    ) -> Result<BinaryResponse, EngineError> {
        if spec.method == Method::GET {
            self.encoding
                .query(&mut spec.url, &spec.inputs, spec.raw_query.as_deref())?;
            spec.inputs.clear();
            spec.raw_query = None;
        }
        for redirects in 0..=MAX_REDIRECTS {
            // Reject a foreign origin before parsing or resolving its address.
            // A redirect never inherits credentials into another origin.
            if Origin::from_url(&spec.url) != Origin::from_url(&self.base_url) {
                return Err(EngineError::OutboundUrlRejected(
                    "cross-origin request".to_owned(),
                ));
            }
            self.ensure_allowed(&spec.url)?;
            self.request_gate.wait().await;
            let (status, headers, body) = self.send_binary(&spec).await?;
            if body.len() > MAX_DIRECT_RESPONSE_BYTES {
                return Err(EngineError::ResponseTooLarge);
            }
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
                && (magnet_redirect
                    || self
                        .definition
                        .spec
                        .implementation
                        .follow_redirects
                        .unwrap_or(true))
            {
                if redirects == MAX_REDIRECTS {
                    return Err(EngineError::TooManyRedirects);
                }
                let location = headers
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(EngineError::RedirectWithoutLocation)?;
                let next = spec.url.join(location)?;
                if magnet_redirect && next.scheme() == "magnet" {
                    super::download::validate_magnet(next.as_str())?;
                    return Ok(BinaryResponse {
                        status: StatusCode::OK,
                        body: next.as_str().as_bytes().to_vec(),
                        final_url: next,
                    });
                }
                // 307/308 preserve POST and its form data. 303 always switches
                // to GET; 301/302 follow browser semantics for POST.
                if status == StatusCode::SEE_OTHER
                    || (spec.method == Method::POST && matches!(status.as_u16(), 301 | 302))
                {
                    spec.method = Method::GET;
                    spec.inputs.clear();
                    spec.headers.retain(|(name, _)| {
                        name != reqwest::header::CONTENT_TYPE
                            && name != reqwest::header::CONTENT_LENGTH
                    });
                }
                spec.url = next;
                continue;
            }
            if !status.is_success() {
                return Err(EngineError::UnexpectedStatus(status.as_u16()));
            }
            return Ok(BinaryResponse {
                status,
                body,
                final_url: spec.url,
            });
        }
        Err(EngineError::TooManyRedirects)
    }

    async fn send_binary(
        &self,
        spec: &RequestSpec,
    ) -> Result<(StatusCode, reqwest::header::HeaderMap, Vec<u8>), EngineError> {
        // Validate before either the production connector or mock transport.
        for (name, _) in &spec.headers {
            if *name == reqwest::header::COOKIE {
                return Err(EngineError::InvalidSearchDefinition(
                    "Cookie headers must use the scoped session jar",
                ));
            }
            if matches!(
                name.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "proxy-authorization"
                    | "connection"
                    | "upgrade"
                    | "proxy-connection"
            ) {
                return Err(EngineError::InvalidDownloadDefinition(
                    "unsafe transport header",
                ));
            }
        }
        #[cfg(test)]
        if let Some(transport) = &self.transport {
            let result = transport.send(spec.clone()).await?;
            // Test transports must exercise the exact cookie-import behavior
            // of reqwest, otherwise mocked login flows cannot model sessions.
            self.engine.cookies.set_cookies(
                &mut result.1.get_all(reqwest::header::SET_COOKIE).iter(),
                &spec.url,
            );
            return Ok(result);
        }
        let profile = DirectRequestProfile {
            user_agent: self.profile.user_agent.clone(),
            cookie_header: None,
        };
        let mut request = self
            .engine
            .request(spec.method.clone(), spec.url.clone(), &profile);
        for (name, value) in &spec.headers {
            request = request.header(name, value);
        }
        if spec.method == Method::POST {
            let mut body = self.encoding.form(&spec.inputs)?;
            if let Some(raw) = spec.raw_query.as_deref().filter(|raw| !raw.is_empty()) {
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
        let mut response = request.send().await.map_err(classify_http_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|v| v > MAX_DIRECT_RESPONSE_BYTES as u64)
        {
            return Err(EngineError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(classify_http_error)? {
            if body.len().saturating_add(chunk.len()) > MAX_DIRECT_RESPONSE_BYTES {
                return Err(EngineError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, headers, body))
    }
}

fn classify_http_error(error: reqwest::Error) -> EngineError {
    if super::mirrors::unsafe_dns_error(&error) {
        EngineError::OutboundAddressRejected
    } else {
        EngineError::HttpClient(error.without_url())
    }
}
