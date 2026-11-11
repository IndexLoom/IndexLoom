//! Verified, versioned Indexers release ingestion (JSONL/gzip transport).
//! SQLite is the authoritative distribution format; JSONL is a lossless
//! transport exported from the same build so the application can reuse its
//! existing, migration-managed database without importing a foreign SQLite file.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

use flate2::read::GzDecoder;
use indexloom_definitions::{SourceProject, parse_yaml};
use indexloom_storage::{CatalogDefinitionInput, CatalogSyncReport, Store};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    CatalogError, MAX_EXTRACTED_BYTES, MAX_RELEASE_MANIFEST_BYTES, download_archive,
    download_with_limit,
};

const RELEASE_FORMAT: &str = "indexloom-indexers/v1";
const RELEASE_ASSET: &str = "indexers.jsonl.gz";
const MAX_RELEASE_DEFINITIONS: usize = 20_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseManifest {
    format: String,
    schema_version: String,
    definition_count: usize,
    assets: BTreeMap<String, ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    sha256: String,
    bytes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDefinition {
    id: String,
    definition_yaml: String,
    definition_sha256: String,
}

pub(super) async fn synchronize(
    store: &Store,
    manifest_url: &Url,
) -> Result<CatalogSyncReport, CatalogError> {
    let manifest_bytes = download_with_limit(manifest_url, MAX_RELEASE_MANIFEST_BYTES).await?;
    let fingerprint = format!("{:x}", Sha256::digest(&manifest_bytes));
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| CatalogError::Release(format!("invalid release manifest: {error}")))?;
    if manifest.format != RELEASE_FORMAT
        || manifest.schema_version != indexloom_definitions::API_VERSION
        || manifest.definition_count == 0
        || manifest.definition_count > MAX_RELEASE_DEFINITIONS
    {
        return Err(CatalogError::Release(
            "unsupported release format or definition count".into(),
        ));
    }
    if store
        .catalog_release_fingerprint(manifest_url.as_str())
        .await
        .map_err(CatalogError::Storage)?
        .as_deref()
        == Some(fingerprint.as_str())
    {
        let retained = store
            .definition_count()
            .await
            .map_err(CatalogError::Storage)?;
        if retained > 0 {
            return Ok(CatalogSyncReport {
                definitions: usize::try_from(retained).map_err(|_| {
                    CatalogError::Release("stored definition count is out of range".into())
                })?,
                new_indexers: 0,
                updated: false,
            });
        }
    }

    let asset = manifest.assets.get(RELEASE_ASSET).ok_or_else(|| {
        CatalogError::Release("release manifest is missing indexers.jsonl.gz".into())
    })?;
    let asset_url = manifest_url
        .join(RELEASE_ASSET)
        .map_err(|error| CatalogError::Release(format!("invalid release asset URL: {error}")))?;
    let compressed = download_archive(&asset_url).await?;
    if compressed.len() != asset.bytes
        || format!("{:x}", Sha256::digest(&compressed)) != asset.sha256
    {
        return Err(CatalogError::Release(
            "release asset checksum mismatch".into(),
        ));
    }
    let definitions = decode_release_records(&compressed, manifest.definition_count)?;
    // Persist only a completely verified snapshot. Existing settings, secrets
    // and enabled flags are preserved by the catalog storage implementation.
    let report = store
        .sync_catalog(&definitions)
        .await
        .map_err(CatalogError::Storage)?;
    store
        .record_catalog_release_fingerprint(manifest_url.as_str(), &fingerprint)
        .await
        .map_err(CatalogError::Storage)?;
    Ok(report)
}

/// Decode and validate the complete export before mutating the application's SQLite.
fn decode_release_records(
    compressed: &[u8],
    expected_count: usize,
) -> Result<Vec<CatalogDefinitionInput>, CatalogError> {
    let mut decoded = Vec::new();
    GzDecoder::new(compressed)
        .take(MAX_EXTRACTED_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| CatalogError::Release(format!("unable to decode release: {error}")))?;
    if decoded.len() > MAX_EXTRACTED_BYTES {
        return Err(CatalogError::Release(
            "decompressed release exceeds size limit".into(),
        ));
    }
    let records = String::from_utf8(decoded)
        .map_err(|_| CatalogError::Release("JSONL is not UTF-8".into()))?;
    let mut seen = BTreeSet::new();
    let mut definitions = Vec::with_capacity(expected_count);
    for line in records.lines() {
        if line.trim().is_empty() {
            return Err(CatalogError::Release("empty JSONL record".into()));
        }
        let entry: ReleaseDefinition = serde_json::from_str(line)
            .map_err(|error| CatalogError::Release(format!("invalid release record: {error}")))?;
        if !seen.insert(entry.id.clone()) {
            return Err(CatalogError::Release(format!(
                "duplicate definition ID: {}",
                entry.id
            )));
        }
        let digest = format!("{:x}", Sha256::digest(entry.definition_yaml.as_bytes()));
        if digest != entry.definition_sha256 {
            return Err(CatalogError::Release(format!(
                "definition checksum mismatch: {}",
                entry.id
            )));
        }
        let definition = parse_yaml(&entry.definition_yaml).map_err(CatalogError::Definition)?;
        if definition.metadata.id != entry.id {
            return Err(CatalogError::Release(format!(
                "definition ID mismatch: {}",
                entry.id
            )));
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
            content_yaml: entry.definition_yaml,
            content_sha256: digest,
        });
    }
    if definitions.len() != expected_count {
        return Err(CatalogError::Release(format!(
            "release count mismatch: expected {}, received {}",
            expected_count,
            definitions.len()
        )));
    }
    Ok(definitions)
}

#[cfg(test)]
mod tests {
    use super::{RELEASE_ASSET, RELEASE_FORMAT, ReleaseDefinition, ReleaseManifest};

    #[test]
    fn understands_database_export_contract() {
        let manifest: ReleaseManifest = serde_json::from_str(
            r#"{"format":"indexloom-indexers/v1","schemaVersion":"indexloom.io/v1alpha1","definitionCount":1,"assets":{"indexers.jsonl.gz":{"sha256":"abc","bytes":12}}}"#,
        ).unwrap();
        assert_eq!(manifest.format, RELEASE_FORMAT);
        assert_eq!(manifest.assets[RELEASE_ASSET].bytes, 12);
        let entry: ReleaseDefinition = serde_json::from_str(
            r#"{"id":"example","definitionYaml":"hello","definitionSha256":"abc","definition":{"metadata":{"id":"example"}}}"#,
        ).unwrap();
        assert_eq!(entry.id, "example");
    }
}
