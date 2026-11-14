//! Independently implemented `FileList`, UNIT3D and `AvistaZ` native API adapters.
//! Network I/O is exclusively delegated to [`ValidatedTransport`].
#![doc = include_str!("../README.md")]

mod adapter;
mod avistaz;
mod filelist;
mod model;
mod parse;
mod torrent;
mod transport;
mod unit3d;

pub use adapter::NativeAdapter;
pub use model::*;
pub use transport::*;

use async_trait::async_trait;

/// Stable, redacted errors: upstream messages/bodies/URLs are never included.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid adapter configuration: {0}")]
    Configuration(&'static str),
    #[error("unsupported or invalid query: {0}")]
    Query(&'static str),
    #[error("authentication failed")]
    Authentication,
    #[error("API rejected the request")]
    Api,
    #[error("rate limited")]
    RateLimited,
    #[error("unexpected HTTP status {0}")]
    Http(u16),
    #[error("redirect refused")]
    Redirect,
    #[error("unsafe or foreign-origin URL refused")]
    UnsafeUrl,
    #[error("download handle belongs to a different adapter instance")]
    ForeignRelease,
    #[error("invalid response: {0}")]
    Parse(&'static str),
    #[error("request timed out")]
    Timeout,
    #[error("response exceeded the byte limit")]
    BodyTooLarge,
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Host integration interface. Store one object per configured tracker instance.
/// Calls on an instance are serialized to isolate authentication and pacing.
#[async_trait]
pub trait NativeIndexer: Send + Sync {
    fn site_id(&self) -> &'static str;
    fn instance_id(&self) -> &str;
    async fn search(&self, query: &NativeQuery) -> Result<SearchPage, Error>;
    async fn download(&self, release: &NativeRelease) -> Result<TorrentFile, Error>;
}
