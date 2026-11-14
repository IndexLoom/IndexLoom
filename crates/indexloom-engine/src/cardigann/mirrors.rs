//! Catalog-only failover. Never retries an authentication or origin-policy error.
use super::{
    AuthSessionState, CardigannExecutor, DirectRequestProfile, Duration, EngineError,
    IndexerDefinition, IndexerSearchBatch, Instant, Origin, ProxyRoute, SearchEngine,
    SearchParameters, TemplateValue, Url, validate_public_url_literal,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MAX_ATTEMPTS: usize = 3;
const COOLDOWN: Duration = Duration::from_secs(60);
type Runtime = (
    SearchEngine,
    Arc<tokio::sync::Mutex<AuthSessionState>>,
    Arc<super::transport::RequestGate>,
    Arc<tokio::sync::Mutex<()>>,
);

#[derive(Debug)]
pub(super) struct MirrorPool {
    urls: Vec<Url>,
    state: Mutex<MirrorHealth>,
    runtimes: Mutex<HashMap<usize, Runtime>>,
    proxy: Option<ProxyRoute>,
}

#[derive(Debug, Default)]
struct MirrorHealth {
    preferred: usize,
    successful: Option<usize>,
    failed: HashMap<usize, Instant>,
}

impl MirrorPool {
    pub(super) fn new(
        definition: &IndexerDefinition,
        proxy: Option<&ProxyRoute>,
    ) -> Result<Self, EngineError> {
        let mut urls = Vec::new();
        for value in definition
            .spec
            .base_urls
            .iter()
            .chain(&definition.spec.legacy_urls)
        {
            let url = Url::parse(value)?;
            validate_public_url_literal(&url)?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(EngineError::OutboundAddressRejected);
            }
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        Ok(Self {
            urls,
            state: Mutex::default(),
            runtimes: Mutex::default(),
            proxy: proxy.cloned(),
        })
    }

    fn candidates(&self) -> Vec<usize> {
        let health = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut candidates: Vec<_> = (0..self.urls.len()).collect();
        candidates.sort_by_key(|i| {
            (
                health
                    .failed
                    .get(i)
                    .is_some_and(|time| time.elapsed() < COOLDOWN),
                Some(*i) != health.successful,
                *i != health.preferred,
                *i,
            )
        });
        candidates.truncate(MAX_ATTEMPTS);
        candidates
    }

    fn record(&self, index: usize, success: bool) {
        let mut health = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if success {
            health.successful = Some(index);
            health.failed.remove(&index);
        } else {
            health.failed.insert(index, Instant::now());
        }
    }
}

impl CardigannExecutor {
    /// Selects an exact URL already declared by this definition. Does not extend
    /// the outbound allowlist. The choice lasts for this cached executor.
    ///
    /// # Errors
    /// Returns [`EngineError::OutboundUrlRejected`] for an undeclared URL.
    pub fn set_preferred_mirror(&self, url: &Url) -> Result<(), EngineError> {
        let index = self
            .mirrors
            .urls
            .iter()
            .position(|candidate| candidate == url)
            .ok_or_else(|| {
                EngineError::OutboundUrlRejected("undeclared preferred mirror".to_owned())
            })?;
        let mut health = self
            .mirrors
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        health.preferred = index;
        health.successful = None;
        health.failed.remove(&index);
        Ok(())
    }

    fn on_mirror(&self, index: usize) -> Result<Self, EngineError> {
        let base_url = self.mirrors.urls[index].clone();
        // A pasted browser cookie is scoped to its original origin. Unlike a
        // definition's login form, it cannot establish a session at a new host.
        if self.configured_cookie
            && Origin::from_url(&base_url) != Origin::from_url(&self.mirrors.urls[0])
        {
            return Err(EngineError::AuthenticationRejected);
        }
        let (engine, auth_state, request_gate, operation) = {
            let mut runtimes = self
                .mirrors
                .runtimes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(runtime) = runtimes.get(&index) {
                runtime.clone()
            } else {
                let runtime = if index == 0 {
                    (
                        self.engine.clone(),
                        self.auth_state.clone(),
                        self.request_gate.clone(),
                        self.operation.clone(),
                    )
                } else {
                    (
                        SearchEngine::with_proxy_and_roots(
                            self.mirrors.proxy.as_ref(),
                            super::transport::certificate_roots(
                                &self.definition.spec.implementation.certificates,
                            )?,
                        )?,
                        Arc::new(tokio::sync::Mutex::new(AuthSessionState::default())),
                        Arc::new(super::transport::RequestGate::new(
                            self.definition.spec.implementation.request_delay_seconds,
                        )?),
                        Arc::new(tokio::sync::Mutex::new(())),
                    )
                };
                runtimes.insert(index, runtime.clone());
                runtime
            }
        };
        let mut context = self.context.clone();
        context.config.insert(
            "sitelink".to_owned(),
            TemplateValue::String(base_url.to_string()),
        );
        Ok(Self {
            indexer_instance_id: self.indexer_instance_id.clone(),
            definition: self.definition.clone(),
            context,
            engine,
            profile: DirectRequestProfile {
                user_agent: self.profile.user_agent.clone(),
                cookie_header: self.profile.cookie_header.clone(),
            },
            proxy: self.proxy.clone(),
            flaresolverr: self.flaresolverr.clone(),
            base_url,
            allowed_origins: self.allowed_origins.clone(),
            auth_state,
            encoding: super::transport::WireEncoding::new(
                &self.definition.spec.implementation.encoding,
            )?,
            request_gate,
            operation,
            persistence: if index == 0 {
                self.persistence.clone()
            } else {
                None
            },
            configured_cookie: self.configured_cookie && index == 0,
            mirrors: self.mirrors.clone(),
            #[cfg(test)]
            transport: self.transport.clone(),
        })
    }

    pub(super) async fn search_page_mirrors(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
        parameters: &SearchParameters,
    ) -> Result<IndexerSearchBatch, EngineError> {
        let term = term.to_owned();
        let parameters = parameters.clone();
        self.with_mirrors(|executor| {
            let term = term.clone();
            let parameters = parameters.clone();
            Box::pin(async move {
                executor
                    .search_page_single(&term, limit, offset, &parameters)
                    .await
            })
        })
        .await
    }

    pub(super) async fn test_mirrors(&self) -> Result<String, EngineError> {
        self.with_mirrors(|executor| Box::pin(async move { executor.test_single().await }))
            .await
    }

    pub(super) async fn with_mirrors<'a, T, F>(&'a self, operation: F) -> Result<T, EngineError>
    where
        F: for<'b> Fn(
            &'b Self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, EngineError>> + Send + 'b>,
        >,
    {
        tokio::time::timeout(Duration::from_secs(90), async {
            let mut last = EngineError::MissingBaseUrl;
            for index in self.mirrors.candidates() {
                let executor = self.on_mirror(index)?;
                match operation(&executor).await {
                    Ok(value) => {
                        self.mirrors.record(index, true);
                        return Ok(value);
                    }
                    Err(error) if retryable(&error) => {
                        self.mirrors.record(index, false);
                        last = error;
                    }
                    Err(error) => {
                        if matches!(
                            error,
                            EngineError::AuthenticationRejected
                                | EngineError::UnexpectedStatus(401 | 403)
                        ) {
                            executor.auth_state.lock().await.last_success = None;
                        }
                        return Err(error);
                    }
                }
            }
            Err(last)
        })
        .await
        .map_err(|_| EngineError::DownloadBudgetExceeded)?
    }

    pub(super) fn mirror_release_url(&self, source: &Url) -> Result<Url, EngineError> {
        self.ensure_allowed(source)?;
        let origin = Origin::from_url(source);
        let base = self
            .mirrors
            .urls
            .iter()
            .filter(|base| {
                Origin::from_url(base) == origin && source.path().starts_with(base.path())
            })
            .max_by_key(|base| base.path().len())
            .ok_or_else(|| EngineError::OutboundUrlRejected("unknown release origin".to_owned()))?;
        let relative = source
            .path()
            .strip_prefix(base.path())
            .ok_or(EngineError::DownloadNotFound)?;
        let mut target = self.base_url.clone();
        target.set_path(&format!("{}{}", self.base_url.path(), relative));
        target.set_query(source.query());
        Ok(target)
    }
}

pub(super) fn unsafe_dns_error(error: &reqwest::Error) -> bool {
    use std::error::Error;
    let mut source = error.source();
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
            || error.to_string().to_ascii_lowercase().contains("dns error")
        {
            // A failed DNS resolution may be a deliberate private-IP block:
            // never retry to another mirror with tracker credentials.
            return true;
        }
        source = error.source();
    }
    false
}

pub(super) fn retryable(error: &EngineError) -> bool {
    match error {
        EngineError::RequestTimeout => true,
        EngineError::HttpClient(error) => {
            !unsafe_dns_error(error)
                && (error.is_timeout() || error.is_connect() || error.is_body())
        }
        EngineError::UnexpectedStatus(status) => {
            matches!(status, 408 | 500 | 502 | 503 | 504 | 521 | 522 | 523 | 524)
        }
        _ => false,
    }
}
