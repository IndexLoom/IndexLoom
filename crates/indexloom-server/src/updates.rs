//! GitHub-only release discovery and explicit verified package staging. No execution or extraction.
use crate::{ApiError, AppState};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path, time::Duration};
fn current_version() -> &'static str {
    option_env!("INDEXLOOM_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
fn compare_tags(left: &str, right: &str) -> std::cmp::Ordering {
    match (version(left), version(right)) {
        (Some(left), Some(right)) => left.cmp_precedence(&right),
        (left, right) => left.cmp(&right),
    }
}
const MAX_METADATA: usize = 8 * 1024 * 1024;
const MAX_PACKAGE: usize = 512 * 1024 * 1024;
const REFRESH_SECONDS: i64 = 300;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub body: Option<String>,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Deserialize, Default)]
struct Options {
    #[serde(default)]
    prerelease: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct History {
    current_version: &'static str,
    repository: String,
    checked_at: i64,
    releases: Vec<Release>,
    verification: &'static str,
    install_policy: &'static str,
}
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/system/updates", get(latest))
        .route("/api/v1/system/updates/history", get(history))
}
fn repository() -> Result<String> {
    let repo = std::env::var("INDEXLOOM_UPDATE_REPOSITORY")
        .unwrap_or_else(|_| "IndexLoom/IndexLoom".into());
    validate_repository(&repo)?;
    Ok(repo)
}
fn validate_repository(repo: &str) -> Result<()> {
    let parts: Vec<_> = repo.split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|p| !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))),
        "expected GitHub owner/repository"
    );
    Ok(())
}
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("IndexLoom/", env!("CARGO_PKG_VERSION")))
        .build()?)
}
fn safe_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}
async fn response(client: &reqwest::Client, source: &str) -> Result<reqwest::Response> {
    let mut url = url::Url::parse(source)?;
    for _ in 0..5 {
        ensure!(
            safe_url(&url),
            "release URL is outside trusted GitHub hosts"
        );
        let response = client
            .get(url.clone())
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;
        if response.status().is_redirection() {
            url = url.join(
                response
                    .headers()
                    .get("location")
                    .context("redirect without location")?
                    .to_str()?,
            )?;
        } else {
            return Ok(response.error_for_status()?);
        }
    }
    bail!("too many release redirects")
}
async fn bytes(client: &reqwest::Client, source: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = response(client, source).await?;
    ensure!(
        response.content_length().unwrap_or(0) <= limit as u64,
        "release response too large"
    );
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            data.len().saturating_add(chunk.len()) <= limit,
            "release response too large"
        );
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
async fn fetch(repo: &str) -> Result<Vec<Release>> {
    validate_repository(repo)?;
    let data = bytes(
        &client()?,
        &format!("https://api.github.com/repos/{repo}/releases?per_page=100"),
        MAX_METADATA,
    )
    .await?;
    let mut releases: Vec<Release> = serde_json::from_slice(&data)?;
    releases.retain(|r| !r.draft);
    releases.sort_by(|a, b| {
        compare_tags(&b.tag_name, &a.tag_name).then_with(|| b.published_at.cmp(&a.published_at))
    });
    Ok(releases)
}
/// `SemVer` also orders numeric prerelease/date/run components (e.g. nightly.20261114.10).
fn version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}
async fn cached(state: &AppState, prerelease: bool) -> Result<History> {
    let repo = repository()?;
    // Serialize refreshes and re-read persistent cache under the lock to bound API traffic.
    let mut guard = state.update_lock.lock().await;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let saved = state.store.update_cache(&repo).await?;
    let (checked_at, mut releases): (i64, Vec<Release>) = match saved {
        Some((at, document)) if now.saturating_sub(at) < REFRESH_SECONDS => {
            (at, serde_json::from_str(&document)?)
        }
        previous => {
            if guard.is_some_and(|at| at.elapsed() < Duration::from_secs(300)) {
                if let Some((at, document)) = previous {
                    (at, serde_json::from_str(&document)?)
                } else {
                    bail!("update refresh temporarily unavailable; retry in five minutes");
                }
            } else {
                *guard = Some(std::time::Instant::now());
                match fetch(&repo).await {
                    Ok(releases) => {
                        state
                            .store
                            .save_update_cache(&repo, now, &serde_json::to_string(&releases)?)
                            .await?;
                        (now, releases)
                    }
                    Err(error) => {
                        if let Some((at, document)) = previous {
                            (at, serde_json::from_str(&document)?)
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
    };
    releases.retain(|r| prerelease || !r.prerelease);
    Ok(History {
        current_version: current_version(),
        repository: repo,
        checked_at,
        releases,
        verification: "Metadata only; packages are verified against SHA256SUMS on explicit CLI download. Checksums are not publisher signatures.",
        install_policy: "Manual installation only. Docker: recreate image or use your managed Watchtower policy. iOS: signed platform distribution only.",
    })
}
async fn history(
    State(state): State<AppState>,
    Query(options): Query<Options>,
) -> Result<Json<History>, ApiError> {
    cached(&state, options.prerelease)
        .await
        .map(Json)
        .map_err(|error| {
            tracing::warn!(%error, "update check failed");
            ApiError::internal("update metadata unavailable")
        })
}
async fn latest(
    State(state): State<AppState>,
    Query(options): Query<Options>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Json(history) = history(State(state), Query(options)).await?;
    let latest = history
        .releases
        .iter()
        .find(|r| version(&r.tag_name).is_some());
    let available = latest.and_then(|r| version(&r.tag_name)).is_some_and(|v| {
        version(current_version()).is_some_and(|current| v.cmp_precedence(&current).is_gt())
    });
    Ok(Json(
        serde_json::json!({"currentVersion": history.current_version, "latest": latest, "updateAvailable": available, "checkedAt": history.checked_at, "verification": history.verification, "installPolicy": history.install_policy}),
    ))
}
fn checksum(document: &str, name: &str) -> Result<String> {
    let matches: Vec<_> = document
        .lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim_start().trim_start_matches('*') == name
                && hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
        })
        .collect();
    ensure!(matches.len() == 1, "missing or ambiguous SHA256SUMS entry");
    Ok(matches[0].clone())
}
fn stage_verified(data: &[u8], size: u64, expected: &str, destination: &Path) -> Result<()> {
    ensure!(data.len() as u64 == size, "package size mismatch");
    ensure!(
        format!("{:x}", Sha256::digest(data)) == expected,
        "SHA256 mismatch; nothing written"
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .context("destination must be a new file")?;
    if let Err(error) = file.write_all(data).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(destination);
        return Err(error.into());
    }
    Ok(())
}
/// Runs only when the first argument is `update`; does not initialize normal server state.
///
/// # Errors
/// Fails closed on invalid selection, network failure, missing checksums or an existing destination.
pub async fn cli(args: &[String]) -> Result<()> {
    let repo = repository()?;
    let prerelease = args.iter().any(|a| a == "--prerelease");
    match args.first().map(String::as_str) {
        Some("check") if args.len() == 1 || args.len() == 2 && prerelease => {
            let releases: Vec<_> = fetch(&repo)
                .await?
                .into_iter()
                .filter(|r| prerelease || !r.prerelease)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"currentVersion": current_version(), "latest": releases.first(), "updateAvailable": releases.first().and_then(|r| version(&r.tag_name)).is_some_and(|v| version(current_version()).is_some_and(|current| v.cmp_precedence(&current).is_gt())), "releases": releases, "verification": "Metadata only; use update download for SHA256 verification"})
                )?
            );
        }
        Some("download") if args.len() == 4 => {
            ensure!(
                !cfg!(any(target_os = "android", target_os = "ios")),
                "mobile updates require platform distribution; use release links"
            );
            let releases = fetch(&repo).await?;
            let release = releases
                .iter()
                .find(|r| r.tag_name == args[1])
                .context("release tag not found")?;
            let asset = release
                .assets
                .iter()
                .find(|a| a.name == args[2])
                .context("asset not found")?;
            ensure!(
                !Path::new(&asset.name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ipa")),
                "iOS packages require platform signing/distribution; use release links"
            );
            ensure!(asset.size <= MAX_PACKAGE as u64, "package exceeds 512 MiB");
            let sums = release
                .assets
                .iter()
                .find(|a| a.name == "SHA256SUMS" || a.name == "SHA256SUMS.txt")
                .context("release has no SHA256SUMS")?;
            let prefix = format!("https://github.com/{repo}/releases/download/");
            ensure!(
                asset.browser_download_url.starts_with(&prefix)
                    && sums.browser_download_url.starts_with(&prefix),
                "asset does not belong to configured repository"
            );
            let client = client()?;
            let expected = checksum(
                std::str::from_utf8(
                    &bytes(&client, &sums.browser_download_url, MAX_METADATA).await?,
                )?,
                &asset.name,
            )?;
            let data = bytes(&client, &asset.browser_download_url, MAX_PACKAGE).await?;
            stage_verified(&data, asset.size, &expected, Path::new(&args[3]))?;
            println!(
                "Verified SHA256 {expected}. Package staged; install manually after stopping the application. No binary replaced."
            );
        }
        _ => bail!(
            "usage: indexloom-server update check [--prerelease] | update download TAG ASSET NEW_DESTINATION"
        ),
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_and_checksums() {
        assert!(compare_tags("v0.3.0-dev.20261114.10", "v0.3.0-dev.20261114.9").is_gt());
        assert!(compare_tags("0.3.0+build.9", "0.3.0+build.10").is_eq());
        assert!(version("v0.10.0") > version("v0.9.9"));
        assert!(version("0.3.1-nightly.20261114.10") > version("0.3.1-nightly.20261114.9"));
        assert!(version("0.3.1") > version("0.3.1-rc.2"));
        assert!(checksum(&format!("{} *app.zip", "a".repeat(64)), "app.zip").is_ok());
        assert!(checksum("bad app.zip", "app.zip").is_err());
        assert!(
            checksum(
                &format!("{0} app.zip\n{0} app.zip", "a".repeat(64)),
                "app.zip"
            )
            .is_err()
        );
    }
    #[test]
    fn staging_verifies_before_write_and_never_overwrites() {
        let path =
            std::env::temp_dir().join(format!("indexloom-update-test-{}", uuid::Uuid::now_v7()));
        let data = b"verified package";
        let digest = format!("{:x}", Sha256::digest(data));
        assert!(stage_verified(data, data.len() as u64, &"0".repeat(64), &path).is_err());
        assert!(!path.exists());
        assert!(stage_verified(data, 1, &digest, &path).is_err());
        assert!(!path.exists());
        stage_verified(data, data.len() as u64, &digest, &path).unwrap();
        assert!(
            stage_verified(
                b"other",
                5,
                &format!("{:x}", Sha256::digest(b"other")),
                &path
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), data);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn rejects_untrusted_urls_and_repositories() {
        for url in [
            "http://github.com/a",
            "https://localhost/a",
            "https://github.com.evil/a",
            "https://user@github.com/a",
            "https://github.com:444/a",
        ] {
            assert!(!safe_url(&url::Url::parse(url).unwrap()));
        }
        for repo in ["../x", "a/b/c", "a/b?x", "a/"] {
            assert!(validate_repository(repo).is_err());
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[tokio::test]
    async fn sqlite_cache_survives_state_recreation_and_filters_prereleases() {
        let store = indexloom_storage::Store::connect("sqlite::memory:")
            .await
            .unwrap();
        let document = serde_json::json!([
            {"tag_name":"v0.4.0", "html_url":"https://github.com/IndexLoom/IndexLoom/releases/tag/v0.4.0", "published_at":"2026-11-14T00:00:00Z", "prerelease":false, "draft":false, "body":"Release", "assets":[]},
            {"tag_name":"v0.5.0-dev.20261114.10", "html_url":"https://github.com/IndexLoom/IndexLoom/releases/tag/v0.5.0-dev.20261114.10", "published_at":"2026-11-14T00:00:00Z", "prerelease":true, "draft":false, "body":"Preview", "assets":[]}
        ]);
        store
            .save_update_cache(
                "IndexLoom/IndexLoom",
                time::OffsetDateTime::now_utc().unix_timestamp(),
                &document.to_string(),
            )
            .await
            .unwrap();
        let state = AppState::new(
            store.clone(),
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new("update-test-token-with-at-least-32-bytes").unwrap(),
        );
        assert_eq!(cached(&state, false).await.unwrap().releases.len(), 1);
        assert_eq!(cached(&state, true).await.unwrap().releases.len(), 2);
        let old_timestamp = time::OffsetDateTime::now_utc().unix_timestamp() - 600;
        state
            .store
            .save_update_cache("IndexLoom/IndexLoom", old_timestamp, &document.to_string())
            .await
            .unwrap();
        *state.update_lock.lock().await = Some(std::time::Instant::now());
        assert_eq!(
            cached(&state, false).await.unwrap().checked_at,
            old_timestamp
        );
        state
            .store
            .save_update_cache(
                "IndexLoom/IndexLoom",
                time::OffsetDateTime::now_utc().unix_timestamp(),
                &document.to_string(),
            )
            .await
            .unwrap();
        let state = AppState::new(
            store,
            indexloom_storage::SecretVault::generate().unwrap(),
            crate::AdminToken::new("update-test-token-with-at-least-32-bytes").unwrap(),
        );
        assert_eq!(
            cached(&state, false).await.unwrap().releases[0].tag_name,
            "v0.4.0"
        );
    }
}
