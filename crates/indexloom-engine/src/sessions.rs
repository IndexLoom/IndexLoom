//! A bounded reqwest cookie provider with serializable RFC6265 state.

use std::{
    fmt,
    fmt::Write as _,
    sync::{LazyLock, Mutex},
};

use async_trait::async_trait;
use cookie_store::{Cookie, CookieExpiration, CookieStore};
use reqwest::{cookie::CookieStore as ReqwestCookieStore, header::HeaderValue};
use time::OffsetDateTime;
use url::Url;

use crate::EngineError;

const MAX_COOKIES: usize = 128;
const MAX_COOKIE_BYTES: usize = 4 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
// A tracker login is an application session, rather than the process lifetime.
// Give cookies without Expires/Max-Age a finite lifetime across restarts. Cap
// persistent cookies too, without extending an earlier server deadline.
const MAX_COOKIE_AGE: time::Duration = time::Duration::days(12);

/// Internal encrypted-storage boundary; not an API response type.
pub struct CookieSnapshot {
    pub cookies_json: String,
    pub expires_at: i64,
}

impl fmt::Debug for CookieSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieSnapshot").finish_non_exhaustive()
    }
}

/// Implemented by the host using its encrypted storage and credential revision.
/// Errors must never contain plaintext cookies or credential data.
#[async_trait]
pub trait SessionPersistence: Send + Sync + fmt::Debug {
    async fn load(&self) -> Result<Option<CookieSnapshot>, EngineError>;
    async fn save(&self, snapshot: CookieSnapshot) -> Result<(), EngineError>;
    async fn clear(&self) -> Result<(), EngineError>;
}

static SUFFIX_LIST: LazyLock<publicsuffix::List> = LazyLock::new(|| {
    publicsuffix::List::from_bytes(include_bytes!("../data/public_suffix_list.dat"))
        .expect("bundled public suffix list")
});

fn with_suffix_list(store: CookieStore) -> CookieStore {
    store.with_suffix_list(SUFFIX_LIST.clone())
}

pub(crate) struct SessionJar(Mutex<CookieStore>);

impl Default for SessionJar {
    fn default() -> Self {
        Self(Mutex::new(with_suffix_list(CookieStore::default())))
    }
}

impl fmt::Debug for SessionJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionJar").finish_non_exhaustive()
    }
}

impl SessionJar {
    pub(crate) fn clear(&self) {
        self.0.lock().expect("cookie store lock").clear();
    }

    pub(crate) fn has_cookies(&self) -> bool {
        self.0
            .lock()
            .expect("cookie store lock")
            .iter_unexpired()
            .next()
            .is_some()
    }

    pub(crate) fn earliest_expiry(&self) -> Option<i64> {
        self.0
            .lock()
            .expect("cookie store lock")
            .iter_unexpired()
            .filter_map(|cookie| match cookie.expires {
                CookieExpiration::AtUtc(at) => Some(at.unix_timestamp()),
                CookieExpiration::SessionEnd => None,
            })
            .min()
    }

    /// A configured Cookie header has no domain attributes. Import it once as
    /// host-only cookies for the definition's primary URL, never as headers.
    pub(crate) fn seed(&self, header: &str, url: &Url) {
        for pair in header.split(';') {
            let pair = pair.trim();
            if pair.split_once('=').is_some() && !pair.contains(['\r', '\n']) {
                self.set_cookies(
                    &mut std::iter::once(
                        &HeaderValue::from_str(&format!(
                            "{pair}; Path=/{}",
                            if url.scheme() == "https" {
                                "; Secure"
                            } else {
                                ""
                            }
                        ))
                        .unwrap_or_else(|_| HeaderValue::from_static("")),
                    ),
                    url,
                );
            }
        }
    }

    pub(crate) fn browser_cookies(&self, url: &Url) -> Vec<crate::BrowserCookie> {
        self.0
            .lock()
            .expect("cookie store lock")
            .matches(url)
            .into_iter()
            .filter(|cookie| url.scheme() == "https" || !cookie.secure().unwrap_or(false))
            .map(|cookie| crate::BrowserCookie {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: match &cookie.domain {
                    cookie_store::CookieDomain::Suffix(domain) => Some(format!(".{domain}")),
                    _ => None,
                },
                path: Some(cookie.path.to_string()),
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
                expires: match cookie.expires {
                    CookieExpiration::AtUtc(at) => {
                        Some(time::Duration::seconds(at.unix_timestamp()).as_seconds_f64())
                    }
                    CookieExpiration::SessionEnd => None,
                },
            })
            .collect()
    }

    pub(crate) fn receive_browser_cookies(&self, cookies: &[crate::BrowserCookie], url: &Url) {
        for cookie in cookies {
            // Browser data is external input. Parse it through the same scoped
            // Set-Cookie validation, disallowing attribute/header injection.
            if [&cookie.name, &cookie.value]
                .iter()
                .any(|value| value.contains([';', '\r', '\n']))
            {
                continue;
            }
            // Browser APIs distinguish domain cookies by a leading dot. A
            // returned bare host is host-only; adding Domain would broaden it.
            let domain = match cookie.domain.as_deref() {
                Some(domain) if domain.starts_with('.') => Some(domain),
                Some(domain)
                    if !url
                        .host_str()
                        .is_some_and(|host| host.eq_ignore_ascii_case(domain)) =>
                {
                    continue;
                }
                _ => None,
            };
            let mut raw = format!("{}={}", cookie.name, cookie.value);
            for (attribute, value) in [("Domain", domain), ("Path", cookie.path.as_deref())] {
                if let Some(value) = value {
                    if value.contains([';', '\r', '\n']) {
                        raw.clear();
                        break;
                    }
                    write!(raw, "; {attribute}={value}").expect("writing to a string cannot fail");
                }
            }
            if raw.is_empty() {
                continue;
            }
            if cookie.secure {
                raw.push_str("; Secure");
            }
            if cookie.http_only {
                raw.push_str("; HttpOnly");
            }
            if let Some(expiry) = cookie
                .expires
                .filter(|expiry| expiry.is_finite() && *expiry >= 0.0)
            {
                // Browser expires=-1 represents a session cookie. Positive times
                // are absolute UNIX deadlines, converted once on receipt.
                let now = time::Duration::seconds(OffsetDateTime::now_utc().unix_timestamp())
                    .as_seconds_f64();
                let age = std::time::Duration::try_from_secs_f64(
                    (expiry - now).clamp(0.0, MAX_COOKIE_AGE.as_seconds_f64()),
                )
                .expect("browser cookie age is finite and bounded")
                .as_secs();
                write!(raw, "; Max-Age={age}").expect("writing to a string cannot fail");
            }
            if let Ok(header) = HeaderValue::from_str(&raw) {
                self.set_cookies(&mut std::iter::once(&header), url);
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Result<Option<CookieSnapshot>, EngineError> {
        let store = self.0.lock().expect("cookie store lock");
        let cookies = store.iter_unexpired().cloned().collect::<Vec<_>>();
        let expires_at = cookies
            .iter()
            .filter_map(|cookie| match cookie.expires {
                CookieExpiration::AtUtc(at) => Some(at.unix_timestamp()),
                CookieExpiration::SessionEnd => None,
            })
            .max();
        let Some(expires_at) = expires_at else {
            return Ok(None);
        };
        let cookies_json =
            serde_json::to_string(&cookies).map_err(|_| EngineError::SessionPersistence)?;
        if cookies_json.len() > MAX_SNAPSHOT_BYTES {
            return Err(EngineError::SessionPersistence);
        }
        Ok(Some(CookieSnapshot {
            cookies_json,
            expires_at,
        }))
    }

    pub(crate) fn restore(&self, snapshot: &CookieSnapshot) -> Result<(), EngineError> {
        if snapshot.cookies_json.len() > MAX_SNAPSHOT_BYTES {
            return Err(EngineError::SessionPersistence);
        }
        let cookies: Vec<Cookie<'static>> = serde_json::from_str(&snapshot.cookies_json)
            .map_err(|_| EngineError::SessionPersistence)?;
        if cookies.len() > MAX_COOKIES {
            return Err(EngineError::SessionPersistence);
        }
        let now = OffsetDateTime::now_utc();
        // Do not reparse Set-Cookie on restart: that would restart Max-Age.
        let cookies = cookies
            .into_iter()
            .filter(|cookie| !cookie.is_expired())
            .map(|mut cookie| {
                cookie.expires = match cookie.expires {
                    CookieExpiration::AtUtc(at) => {
                        CookieExpiration::AtUtc(at.min(now + MAX_COOKIE_AGE).min(
                            OffsetDateTime::from_unix_timestamp(snapshot.expires_at).unwrap_or(now),
                        ))
                    }
                    CookieExpiration::SessionEnd => CookieExpiration::AtUtc(now),
                };
                Ok::<_, EngineError>(cookie)
            });
        *self.0.lock().expect("cookie store lock") =
            with_suffix_list(CookieStore::from_cookies(cookies, false)?);
        Ok(())
    }
}

impl ReqwestCookieStore for SessionJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let mut store = self.0.lock().expect("cookie store lock");
        // Physically remove expired entries, including cookie deletion markers.
        let expired = store
            .iter_any()
            .filter(|cookie| cookie.is_expired())
            .map(|cookie| {
                (
                    String::from(&cookie.domain),
                    cookie.path.to_string(),
                    cookie.name().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        for (domain, path, name) in expired {
            store.remove(&domain, &path, &name);
        }
        for header in headers {
            if header.as_bytes().len() > MAX_COOKIE_BYTES {
                continue;
            }
            let Ok(raw) = header.to_str() else { continue };
            let Ok(mut cookie) = Cookie::parse(raw.to_owned(), url) else {
                continue;
            };
            // Enforce modern secure-cookie prefixes as well as RFC6265 scope.
            let secure = cookie.secure().unwrap_or(false);
            if (secure && url.scheme() != "https")
                || (cookie.name().starts_with("__Secure-") && !secure)
                || (cookie.name().starts_with("__Host-")
                    && (!secure || cookie.domain().is_some() || cookie.path() != Some("/")))
            {
                continue;
            }
            if url.scheme() != "https"
                && store.iter_unexpired().any(|old| {
                    old.secure().unwrap_or(false)
                        && old.name() == cookie.name()
                        && old.domain.matches(url)
                })
            {
                continue;
            }
            let limit = OffsetDateTime::now_utc() + MAX_COOKIE_AGE;
            cookie.expires = match cookie.expires {
                CookieExpiration::AtUtc(at) => CookieExpiration::AtUtc(at.min(limit)),
                CookieExpiration::SessionEnd => CookieExpiration::AtUtc(limit),
            };
            // Updates and deletions must still work when the jar is full.
            if store.iter_unexpired().count() >= MAX_COOKIES
                && !store.iter_unexpired().any(|old| {
                    old.name() == cookie.name()
                        && old.domain == cookie.domain
                        && old.path == cookie.path
                })
            {
                continue;
            }
            // Avoid store_response_cookies: that helper logs plaintext values.
            let _ = store.insert(cookie, url);
        }
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self.0.lock().expect("cookie store lock");
        let mut cookies = store.matches(url);
        cookies.retain(|cookie| url.scheme() == "https" || !cookie.secure().unwrap_or(false));
        // More specific paths precede less specific ones with the same name.
        cookies.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        let value = cookies
            .into_iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            return None;
        }
        let mut header = HeaderValue::from_str(&value).ok()?;
        header.set_sensitive(true);
        Some(header)
    }
}

#[cfg(test)]
mod tests;
