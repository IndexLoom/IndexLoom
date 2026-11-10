//! Verified synchronization from the separately versioned Indexers catalog.

mod release;

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use indexloom_definitions::{SourceProject, parse_yaml};
use indexloom_storage::{CatalogDefinitionInput, CatalogSyncReport, Store};
use reqwest::{Client, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: usize = 96 * 1024 * 1024;
const MAX_CATALOG_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Local checkout or remote GitHub archive used as the catalog source.
#[derive(Clone, Debug)]
pub enum CatalogSource {
    Directory(PathBuf),
    Archive(Url),
    /// A separately published release manifest with a verified database export.
    Release(Url),
}

impl CatalogSource {
    /// Builds a source from optional environment-style values.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] for an invalid archive URL.
    pub fn from_values(directory: Option<String>, archive_url: &str) -> Result<Self, CatalogError> {
        if let Some(directory) = directory.filter(|value| !value.trim().is_empty()) {
            return Ok(Self::Directory(PathBuf::from(directory)));
        }
        let url = Url::parse(archive_url).map_err(CatalogError::InvalidArchiveUrl)?;
        if url.scheme() != "https" || url.host_str().is_none() {
            return Err(CatalogError::UnsafeArchiveUrl);
        }
        if url.path().ends_with("/manifest.json") {
            Ok(Self::Release(url))
        } else {
            Ok(Self::Archive(url))
        }
    }
}

/// Downloads/parses a catalog, validates every manifest entry, and syncs SQLite.
///
/// # Errors
///
/// Returns [`CatalogError`] when transport, archive, manifest, definition, or
/// persistence validation fails. No partial database update is committed.
pub async fn synchronize(
    store: &Store,
    source: &CatalogSource,
) -> Result<CatalogSyncReport, CatalogError> {
    let files = match source {
        CatalogSource::Directory(path) => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || read_directory(&path))
                .await
                .map_err(CatalogError::Worker)??
        }
        CatalogSource::Release(url) => return release::synchronize(store, url).await,
        CatalogSource::Archive(url) => {
            let bytes = download_archive(url).await?;
            tokio::task::spawn_blocking(move || read_archive(&bytes))
                .await
                .map_err(CatalogError::Worker)??
        }
    };
    let definitions = validate_catalog(files)?;
    store
        .sync_catalog(&definitions)
        .await
        .map_err(CatalogError::Storage)
}

async fn download_archive(url: &Url) -> Result<Vec<u8>, CatalogError> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(CatalogError::Http)?;
    let mut response = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "application/gzip")
        .send()
        .await
        .map_err(CatalogError::Http)?
        .error_for_status()
        .map_err(CatalogError::Http)?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES as u64)
    {
        return Err(CatalogError::ArchiveTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(CatalogError::Http)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE_BYTES {
            return Err(CatalogError::ArchiveTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn read_archive(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, CatalogError> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let mut extracted = 0_usize;
    for entry in archive.entries().map_err(CatalogError::Archive)? {
        let mut entry = entry.map_err(CatalogError::Archive)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(CatalogError::Archive)?.into_owned();
        let relative = strip_archive_root(&path)?;
        if relative != "catalog.json" && !relative.starts_with("definitions/") {
            continue;
        }
        if entry.size() > MAX_CATALOG_FILE_BYTES {
            return Err(CatalogError::CatalogFileTooLarge(relative));
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(CatalogError::Io)?;
        extracted = extracted.saturating_add(content.len());
        if extracted > MAX_EXTRACTED_BYTES {
            return Err(CatalogError::ExtractedCatalogTooLarge);
        }
        files.insert(relative, content);
    }
    Ok(files)
}

fn read_directory(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, CatalogError> {
    let root = if path.join("catalog.json").is_file() {
        path.to_path_buf()
    } else if path.file_name().is_some_and(|name| name == "definitions") {
        path.parent()
            .ok_or(CatalogError::MissingManifest)?
            .to_path_buf()
    } else {
        return Err(CatalogError::MissingManifest);
    };
    let manifest = fs::read(root.join("catalog.json")).map_err(CatalogError::Io)?;
    if manifest.len() as u64 > MAX_CATALOG_FILE_BYTES {
        return Err(CatalogError::CatalogFileTooLarge("catalog.json".to_owned()));
    }
    let parsed: CatalogManifest =
        serde_json::from_slice(&manifest).map_err(CatalogError::Manifest)?;
    let mut files = BTreeMap::from([("catalog.json".to_owned(), manifest)]);
    let mut total = 0_usize;
    for entry in parsed.definitions {
        validate_relative_path(&entry.path)?;
        let content = fs::read(root.join(&entry.path)).map_err(CatalogError::Io)?;
        total = total.saturating_add(content.len());
        if content.len() as u64 > MAX_CATALOG_FILE_BYTES {
            return Err(CatalogError::CatalogFileTooLarge(entry.path));
        }
        if total > MAX_EXTRACTED_BYTES {
            return Err(CatalogError::ExtractedCatalogTooLarge);
        }
        files.insert(entry.path, content);
    }
    Ok(files)
}

fn strip_archive_root(path: &Path) -> Result<String, CatalogError> {
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(CatalogError::UnsafeArchivePath(path.display().to_string()));
    }
    let relative = path.components().skip(1).collect::<PathBuf>();
    let value = relative
        .to_str()
        .ok_or_else(|| CatalogError::UnsafeArchivePath(path.display().to_string()))?
        .replace('\\', "/");
    validate_relative_path(&value)?;
    Ok(value)
}

fn validate_relative_path(value: &str) -> Result<(), CatalogError> {
    let path = Path::new(value);
    let valid = !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(CatalogError::UnsafeArchivePath(value.to_owned()))
    }
}

fn validate_catalog(
    mut files: BTreeMap<String, Vec<u8>>,
) -> Result<Vec<CatalogDefinitionInput>, CatalogError> {
    let manifest_bytes = files
        .remove("catalog.json")
        .ok_or(CatalogError::MissingManifest)?;
    let manifest: CatalogManifest =
        serde_json::from_slice(&manifest_bytes).map_err(CatalogError::Manifest)?;
    if manifest.catalog_version != 1
        || manifest.definition_schema != indexloom_definitions::API_VERSION
        || manifest.definition_count != manifest.definitions.len()
    {
        return Err(CatalogError::InvalidManifestMetadata);
    }
    let mut definitions = Vec::with_capacity(manifest.definitions.len());
    for entry in manifest.definitions {
        validate_relative_path(&entry.path)?;
        let bytes = files
            .remove(&entry.path)
            .ok_or_else(|| CatalogError::MissingDefinition(entry.path.clone()))?;
        let actual_sha = format!("{:x}", Sha256::digest(&bytes));
        if actual_sha != entry.sha256 {
            return Err(CatalogError::HashMismatch(entry.id));
        }
        let yaml = String::from_utf8(bytes).map_err(|_| CatalogError::NonUtf8(entry.path))?;
        let definition = parse_yaml(&yaml).map_err(CatalogError::Definition)?;
        if definition.metadata.id != entry.id {
            return Err(CatalogError::DefinitionIdMismatch(entry.id));
        }
        let source = match definition.provenance.source.project {
            SourceProject::Jackett => "jackett",
            SourceProject::ProwlarrIndexers => "prowlarr-indexers",
        };
        definitions.push(CatalogDefinitionInput {
            id: definition.metadata.id,
            display_name: definition.metadata.name,
            source: source.to_owned(),
            source_revision: Some(definition.provenance.source.revision),
            schema_version: definition.api_version,
            content_yaml: yaml,
            content_sha256: actual_sha,
        });
    }
    Ok(definitions)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogManifest {
    catalog_version: u32,
    definition_schema: String,
    definition_count: usize,
    definitions: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    id: String,
    path: String,
    sha256: String,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("catalog archive URL is invalid: {0}")]
    InvalidArchiveUrl(url::ParseError),
    #[error("catalog archive URL must use HTTPS")]
    UnsafeArchiveUrl,
    #[error("catalog HTTP request failed: {0}")]
    Http(reqwest::Error),
    #[error("Indexers release validation failed: {0}")]
    Release(String),
    #[error("catalog archive exceeds 64 MiB")]
    ArchiveTooLarge,
    #[error("catalog extracted data exceeds 96 MiB")]
    ExtractedCatalogTooLarge,
    #[error("catalog file is too large: {0}")]
    CatalogFileTooLarge(String),
    #[error("catalog archive is invalid: {0}")]
    Archive(std::io::Error),
    #[error("catalog I/O failed: {0}")]
    Io(std::io::Error),
    #[error("catalog worker failed: {0}")]
    Worker(tokio::task::JoinError),
    #[error("catalog manifest is missing")]
    MissingManifest,
    #[error("catalog manifest is invalid: {0}")]
    Manifest(serde_json::Error),
    #[error("catalog manifest metadata is inconsistent")]
    InvalidManifestMetadata,
    #[error("unsafe catalog archive path: {0}")]
    UnsafeArchivePath(String),
    #[error("catalog definition is missing: {0}")]
    MissingDefinition(String),
    #[error("catalog definition hash mismatch: {0}")]
    HashMismatch(String),
    #[error("catalog definition is not UTF-8: {0}")]
    NonUtf8(String),
    #[error("catalog definition is invalid: {0}")]
    Definition(indexloom_definitions::DefinitionError),
    #[error("catalog definition ID mismatch: {0}")]
    DefinitionIdMismatch(String),
    #[error("catalog storage synchronization failed: {0}")]
    Storage(indexloom_storage::StorageError),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{CatalogError, strip_archive_root, validate_catalog};

    #[test]
    fn rejects_archive_traversal() {
        assert!(strip_archive_root(std::path::Path::new("root/../secret")).is_err());
    }

    #[test]
    fn requires_manifest() {
        assert!(matches!(
            validate_catalog(BTreeMap::new()),
            Err(CatalogError::MissingManifest)
        ));
    }
}
