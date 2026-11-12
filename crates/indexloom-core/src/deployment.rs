//! Deployment mode is independent of any desktop or mobile UI toolkit.
//! The same configuration is used by headless binaries and native clients.
use std::env;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    Autonomous,
    External,
}

impl RuntimeMode {
    /// # Errors
    /// Returns an error if the runtime mode is unrecognized.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "autonomous" | "local" => Ok(Self::Autonomous),
            "external" | "remote" => Ok(Self::External),
            _ => Err("runtime mode must be autonomous or external".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deployment {
    pub mode: RuntimeMode,
    pub remote: Option<Url>,
}

impl Deployment {
    pub fn autonomous() -> Self {
        Self {
            mode: RuntimeMode::Autonomous,
            remote: None,
        }
    }

    /// # Errors
    /// Returns an error unless the remote endpoint is a safe HTTPS site root.
    pub fn external(url: &str) -> Result<Self, String> {
        Ok(Self {
            mode: RuntimeMode::External,
            remote: Some(validate_remote_url(url)?),
        })
    }

    /// # Errors
    /// Returns an error if the environment contains an invalid mode or URL.
    pub fn from_env() -> Result<Self, String> {
        let mode = RuntimeMode::parse(
            &env::var("INDEXLOOM_RUNTIME_MODE").unwrap_or_else(|_| "autonomous".into()),
        )?;
        match mode {
            RuntimeMode::Autonomous => Ok(Self::autonomous()),
            RuntimeMode::External => {
                let url = env::var("INDEXLOOM_REMOTE_URL")
                    .map_err(|_| "external mode requires INDEXLOOM_REMOTE_URL")?;
                Self::external(&url)
            }
        }
    }
}

/// Remote installations must have an HTTPS root URL. Never accept credentials
/// in the URL or a fragment that could be mistaken for server configuration.
/// # Errors
/// Returns an error for unsafe schemes, credentials, non-root paths or malformed URLs.
pub fn validate_remote_url(raw: &str) -> Result<Url, String> {
    if raw.len() > 2048 || raw.trim() != raw {
        return Err("remote URL is empty, oversized or padded".into());
    }
    let mut url = Url::parse(raw).map_err(|_| "remote URL is not valid")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "remote URL must be an HTTPS site root without credentials, path, query or fragment"
                .into(),
        );
    }
    url.set_path("/");
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::{Deployment, RuntimeMode, validate_remote_url};

    #[test]
    fn runtime_modes_are_explicit() {
        assert_eq!(
            RuntimeMode::parse("AUTONOMOUS").unwrap(),
            RuntimeMode::Autonomous
        );
        assert_eq!(
            RuntimeMode::parse("external").unwrap(),
            RuntimeMode::External
        );
        assert!(RuntimeMode::parse("disabled").is_err());
        assert!(Deployment::autonomous().remote.is_none());
    }

    #[test]
    fn remote_endpoint_rejects_unsafe_or_misleading_urls() {
        assert_eq!(
            validate_remote_url("https://example.com").unwrap().as_str(),
            "https://example.com/"
        );
        for value in [
            "http://example.com",
            "file:///etc/passwd",
            "https://u:p@example.com",
            "https://example.com/path",
            "https://example.com/#token",
            "https://example.com/?secret=1",
            " https://example.com",
            "https://example.com@127.0.0.1",
        ] {
            assert!(validate_remote_url(value).is_err(), "accepted {value}");
        }
    }
}
