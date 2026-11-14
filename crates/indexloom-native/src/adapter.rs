use std::{collections::BTreeMap, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::{sync::Mutex, time::Instant};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    Credentials, Error, HttpRequest, HttpResponse, InstanceConfig, Method, NativeIndexer,
    NativeQuery, NativeRelease, SearchPage, Secret, TorrentFile, TransportError,
    ValidatedTransport, avistaz, filelist, parse, torrent, unit3d,
};

#[derive(Debug, Default)]
pub(crate) struct Session {
    pub token: Option<Secret>,
    last_request: Option<Instant>,
}

/// A complete protocol implementation selected by `Credentials`.
pub struct NativeAdapter {
    pub(crate) config: InstanceConfig,
    pub(crate) credentials: Credentials,
    transport: Arc<dyn ValidatedTransport>,
    pub(crate) session_id: Uuid,
    session: Mutex<Session>,
}

impl fmt::Debug for NativeAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeAdapter")
            .field("site_id", &self.site_id())
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl NativeAdapter {
    /// Construction does no I/O. The transport is mandatory; no fallback exists.
    ///
    /// # Errors
    /// Returns `Error::Configuration` for invalid base URLs, credentials, instance
    /// identifiers, or request policies exceeding the documented bounds.
    pub fn new(
        mut config: InstanceConfig,
        credentials: Credentials,
        transport: Arc<dyn ValidatedTransport>,
    ) -> Result<Self, Error> {
        let base = &mut config.base_url;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::Configuration("base URL must be a clean HTTPS URL"));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let p = &config.policy;
        if config.instance_id.is_empty() || config.instance_id.len() > 256 {
            return Err(Error::Configuration("instance ID length"));
        }
        if p.timeout.is_zero()
            || p.timeout > Duration::from_secs(120)
            || !(1..=3).contains(&p.max_attempts)
            || p.retry_delay > Duration::from_secs(30)
            || p.max_retry_after > Duration::from_secs(60)
            || !(1..=8 * 1024 * 1024).contains(&p.max_json_bytes)
            || !(1..=32 * 1024 * 1024).contains(&p.max_torrent_bytes)
            || p.min_interval > Duration::from_secs(60)
        {
            return Err(Error::Configuration(
                "request policy exceeds resource bounds",
            ));
        }
        let validate_secret = |secret: &Secret| {
            if secret.expose().trim().is_empty()
                || secret.expose().len() > 4096
                || secret.expose().chars().any(char::is_control)
            {
                Err(Error::Configuration(
                    "credential length or control character",
                ))
            } else {
                Ok(())
            }
        };
        match &credentials {
            Credentials::FileList { username, passkey } => {
                validate_secret(username)?;
                validate_secret(passkey)?;
                if username.expose().contains(':') {
                    return Err(Error::Configuration("Basic auth username contains colon"));
                }
            }
            Credentials::Unit3d { api_token, .. } => validate_secret(api_token)?,
            Credentials::Avistaz {
                username,
                password,
                pid,
            } => {
                validate_secret(username)?;
                validate_secret(password)?;
                validate_secret(pid)?;
            }
        }
        Ok(Self {
            config,
            credentials,
            transport,
            session_id: Uuid::now_v7(),
            session: Mutex::new(Session::default()),
        })
    }

    pub(crate) fn endpoint(&self, path: &str) -> Result<Url, Error> {
        self.config
            .base_url
            .join(path)
            .map_err(|_| Error::UnsafeUrl)
    }

    /// Same-origin protects credentials; actual IP policy is enforced by transport.
    pub(crate) fn link(&self, value: &str) -> Result<Url, Error> {
        let url = self
            .config
            .base_url
            .join(value)
            .map_err(|_| Error::UnsafeUrl)?;
        if url.scheme() != "https"
            || url.origin() != self.config.base_url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::UnsafeUrl);
        }
        Ok(url)
    }

    pub(crate) fn request(&self, method: Method, url: Url, torrent: bool) -> HttpRequest {
        HttpRequest {
            method,
            url,
            headers: BTreeMap::from([(
                "Accept".into(),
                Secret::new(if torrent {
                    "application/x-bittorrent"
                } else {
                    "application/json"
                }),
            )]),
            body: Zeroizing::new(Vec::new()),
            session_id: self.session_id,
            timeout: self.config.policy.timeout,
            max_response_bytes: if torrent {
                self.config.policy.max_torrent_bytes
            } else {
                self.config.policy.max_json_bytes
            },
        }
    }

    pub(crate) async fn send(
        &self,
        session: &mut Session,
        request: HttpRequest,
    ) -> Result<HttpResponse, Error> {
        self.link(request.url.as_str())?;
        let policy = &self.config.policy;
        let attempts = if request.method == Method::Get {
            policy.max_attempts
        } else {
            1
        };
        let interval = if self.site_id() == "avistaz" {
            policy.min_interval.max(Duration::from_millis(6100))
        } else {
            policy.min_interval
        };
        for attempt in 0..attempts {
            if let Some(last) = session.last_request {
                tokio::time::sleep_until(last + interval).await;
            }
            session.last_request = Some(Instant::now());
            let result =
                tokio::time::timeout(policy.timeout, self.transport.execute(request.clone())).await;
            match result {
                Ok(Ok(response)) => {
                    if response.body.len() > request.max_response_bytes {
                        return Err(Error::BodyTooLarge);
                    }
                    if (300..400).contains(&response.status) {
                        return Err(Error::Redirect);
                    }
                    if matches!(response.status, 429 | 502 | 503 | 504) && attempt + 1 < attempts {
                        let delay = self.retry_after(&response)?;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Ok(response);
                }
                Ok(Err(TransportError::BodyTooLarge)) => return Err(Error::BodyTooLarge),
                Ok(Err(TransportError::Timeout)) | Err(_) => {
                    if attempt + 1 == attempts {
                        return Err(Error::Timeout);
                    }
                }
                Ok(Err(TransportError::Transient)) if attempt + 1 < attempts => {}
                Ok(Err(error)) => return Err(Error::Transport(error)),
            }
            tokio::time::sleep(policy.retry_delay).await;
        }
        unreachable!("validated nonzero attempts")
    }

    fn retry_after(&self, response: &HttpResponse) -> Result<Duration, Error> {
        let Some(value) = response.header("Retry-After") else {
            return Ok(self.config.policy.retry_delay);
        };
        // Do not retry sooner than an unparseable (e.g. HTTP-date) server delay.
        // Returning control lets the host scheduler decide when to retry.
        let seconds = value
            .trim()
            .parse::<u64>()
            .map_err(|_| Error::RateLimited)?;
        let delay = Duration::from_secs(seconds);
        if delay > self.config.policy.max_retry_after {
            return Err(Error::RateLimited);
        }
        Ok(delay.max(self.config.policy.retry_delay))
    }

    fn validate_query(&self, q: &NativeQuery) -> Result<(), Error> {
        if q.term.len() > 1024
            || q.term.chars().any(char::is_control)
            || q.limit > 500
            || !(1..=10_000).contains(&q.page)
            || q.native_categories.len() > 50
            || q.native_categories.contains(&0)
            || q.season.is_some_and(|v| v > 9999)
            || q.episode.is_some_and(|v| v == 0 || v > 9999)
        {
            return Err(Error::Query("resource bounds or invalid category/episode"));
        }
        if let Some(imdb) = &q.imdb_id {
            let digits = imdb.strip_prefix("tt").unwrap_or(imdb);
            if !(1..=12).contains(&digits.len()) || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return Err(Error::Query("IMDb identifier"));
            }
        }
        match &self.credentials {
            Credentials::FileList { .. } if q.page != 1 => {
                Err(Error::Query("FileList has no pagination"))
            }
            Credentials::FileList { .. }
                if q.term.trim().is_empty()
                    && q.imdb_id.is_none()
                    && (q.season.is_some() || q.episode.is_some()) =>
            {
                Err(Error::Query(
                    "FileList episode search needs a term or IMDb ID",
                ))
            }
            Credentials::Avistaz { .. }
                if q.native_categories.iter().any(|v| ![1, 2].contains(v)) =>
            {
                Err(Error::Query(
                    "AvistaZ supports movie (1) and TV (2) categories",
                ))
            }
            Credentials::Avistaz { .. } if q.limit > 50 => Err(Error::Query("AvistaZ limit is 50")),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl NativeIndexer for NativeAdapter {
    fn site_id(&self) -> &'static str {
        self.credentials.site_id()
    }

    fn instance_id(&self) -> &str {
        &self.config.instance_id
    }

    async fn search(&self, query: &NativeQuery) -> Result<SearchPage, Error> {
        self.validate_query(query)?;
        if query.limit == 0 {
            return Ok(SearchPage::default());
        }
        let mut session = self.session.lock().await;
        match &self.credentials {
            Credentials::FileList { username, passkey } => {
                filelist::search(self, &mut session, query, username, passkey).await
            }
            Credentials::Unit3d { api_token, api } => {
                unit3d::search(self, &mut session, query, api_token, *api).await
            }
            Credentials::Avistaz { .. } => avistaz::search(self, &mut session, query).await,
        }
    }

    async fn download(&self, release: &NativeRelease) -> Result<TorrentFile, Error> {
        if release.owner != self.session_id {
            return Err(Error::ForeignRelease);
        }
        let mut session = self.session.lock().await;
        let request = self.request(Method::Get, self.link(release.download.as_str())?, true);
        // API-supplied signed links already carry their own credentials. Only
        // AvistaZ adds its per-instance bearer token; never send Basic API auth
        // to arbitrary download paths.
        let response = if self.site_id() == "avistaz" {
            avistaz::authorized(self, &mut session, request).await?
        } else {
            self.send(&mut session, request).await?
        };
        parse::status(&response)?;
        torrent::validate(&response.body)?;
        Ok(TorrentFile(response.body))
    }
}
