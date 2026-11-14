use std::{fmt, time::Duration};

use indexloom_core::{SearchQuery, SearchResult};
use url::Url;
use uuid::Uuid;

use crate::Secret;

/// All shipped implementations are enabled unconditionally; no feature flags.
pub const SUPPORTED_SITE_IDS: &[&str] = &["filelist", "unit3d", "avistaz"];

#[derive(Debug)]
pub enum Credentials {
    FileList {
        username: Secret,
        passkey: Secret,
    },
    Unit3d {
        api_token: Secret,
        api: Unit3dApi,
    },
    Avistaz {
        username: Secret,
        password: Secret,
        pid: Secret,
    },
}

/// Protocol variants observed in the audited Prowlarr and Jackett sources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit3dApi {
    /// Prowlarr's older API: query `api_token` and boolean freeleech.
    Legacy,
    /// Jackett's UNIT3D 9.2 definitions: bearer auth, percentage freeleech.
    #[default]
    Modern,
}

impl Credentials {
    pub fn site_id(&self) -> &'static str {
        match self {
            Self::FileList { .. } => "filelist",
            Self::Unit3d { .. } => "unit3d",
            Self::Avistaz { .. } => "avistaz",
        }
    }
}

/// Per-attempt resource bounds. Invalid policies are rejected at construction.
#[derive(Clone, Debug)]
pub struct RequestPolicy {
    pub timeout: Duration,
    /// Total GET attempts, including the first, in 1..=3. POST is never retried.
    pub max_attempts: u8,
    pub retry_delay: Duration,
    pub max_retry_after: Duration,
    pub max_json_bytes: usize,
    pub max_torrent_bytes: usize,
    /// Additional pacing; `AvistaZ` always waits at least 6.1 s between attempts.
    pub min_interval: Duration,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_attempts: 3,
            retry_delay: Duration::from_millis(500),
            max_retry_after: Duration::from_secs(30),
            max_json_bytes: 2 * 1024 * 1024,
            max_torrent_bytes: 16 * 1024 * 1024,
            min_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Debug)]
pub struct InstanceConfig {
    pub instance_id: String,
    /// HTTPS origin or HTTPS reverse-proxy directory; no query/fragment/userinfo.
    pub base_url: Url,
    pub policy: RequestPolicy,
}

/// Categories here are explicitly tracker-native IDs, never Torznab IDs.
#[derive(Clone, Debug)]
pub struct NativeQuery {
    pub term: String,
    pub native_categories: Vec<u32>,
    /// 0 returns an empty page without network I/O; maximum 500.
    pub limit: u32,
    /// One bounded page per call, in `1..=10_000`; `FileList` only supports page 1.
    pub page: u32,
    pub imdb_id: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub freeleech_only: bool,
}

impl Default for NativeQuery {
    fn default() -> Self {
        Self {
            term: String::new(),
            native_categories: Vec::new(),
            limit: 50,
            page: 1,
            imdb_id: None,
            season: None,
            episode: None,
            freeleech_only: false,
        }
    }
}

impl NativeQuery {
    /// The caller must map `query.categories` through its definition catalog.
    pub fn from_core(query: &SearchQuery, mapped_native_categories: Vec<u32>) -> Self {
        Self {
            term: query.term.clone(),
            limit: query.limit.unwrap_or(50),
            native_categories: mapped_native_categories,
            ..Self::default()
        }
    }
}

/// Fields not represented by the existing core search-result contract.
#[derive(Clone, Debug, Default)]
pub struct ReleaseMetadata {
    pub files: Option<u64>,
    pub grabs: Option<u64>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub tvdb_id: Option<String>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
    pub internal: bool,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
}

/// A result and an instance-bound download handle, created only by an adapter.
/// Signed links stay out of the core DTO: `result.download_url` is `None`.
/// The host should publish its own authenticated native download route instead.
#[derive(Clone)]
pub struct NativeRelease {
    pub result: SearchResult,
    pub metadata: ReleaseMetadata,
    pub(crate) download: Url,
    pub(crate) owner: Uuid,
}

impl NativeRelease {
    /// Sensitive: may include a passkey; use `NativeIndexer::download` normally.
    pub fn download_url(&self) -> &Url {
        &self.download
    }
}

impl fmt::Debug for NativeRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeRelease")
            .field("id", &self.result.id)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct SearchPage {
    pub releases: Vec<NativeRelease>,
    /// Continuation is requested explicitly; no unbounded automatic pagination.
    pub next_page: Option<u32>,
    /// More rows were returned than the requested limit. Requery with a larger
    /// limit if needed; continuing pages can skip these omitted rows.
    pub truncated: bool,
}

/// Validated bencoded metainfo. Debug excludes bytes, which can contain passkeys.
pub struct TorrentFile(pub(crate) Vec<u8>);

impl TorrentFile {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for TorrentFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorrentFile")
            .field("bytes", &self.0.len())
            .finish()
    }
}
