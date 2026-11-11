//! SQLite-first persistence with encrypted indexer credentials.

use std::{collections::BTreeMap, fmt, str::FromStr, time::Duration};

use base64::prelude::{BASE64_STANDARD, Engine as _};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    FromRow, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroize;

pub mod auth;
mod catalog;
mod routing;

pub use catalog::{CatalogDefinitionInput, CatalogStatus, CatalogSyncReport};
pub use routing::{
    IndexerGroup, IndexerGroupInput, IndexerTestRecord, LastIndexerTest, OutboundProxy,
    OutboundProxyInput, ProxyKind, ProxyPasswordUpdate, ResolvedOutboundProxy,
};

const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const KEY_VERSION: i64 = 1;

/// SQLite storage used by the initial `IndexLoom` release line.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

/// Authenticated encryption boundary for tracker credentials.
#[derive(Clone)]
pub struct SecretVault {
    cipher: XChaCha20Poly1305,
}

impl fmt::Debug for SecretVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretVault")
            .field("cipher", &"<redacted>")
            .finish()
    }
}

impl SecretVault {
    /// Decodes a standard Base64-encoded 256-bit master key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the value is not valid Base64 or is not
    /// exactly 32 bytes.
    pub fn from_base64(encoded: &str) -> Result<Self, StorageError> {
        let mut key = [0_u8; MASTER_KEY_BYTES];
        if let Ok(MASTER_KEY_BYTES) = BASE64_STANDARD.decode_slice(encoded, &mut key) {
            Ok(Self::from_key(key))
        } else {
            key.zeroize();
            Err(StorageError::InvalidMasterKey)
        }
    }

    /// Generates an ephemeral key for tests and one-shot tooling.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the operating system random source fails.
    pub fn generate() -> Result<Self, StorageError> {
        let mut key = [0_u8; MASTER_KEY_BYTES];
        getrandom::fill(&mut key).map_err(|_| StorageError::RandomSource)?;
        Ok(Self::from_key(key))
    }

    fn from_key(mut key: [u8; MASTER_KEY_BYTES]) -> Self {
        let cipher = XChaCha20Poly1305::new((&key).into());
        key.zeroize();
        Self { cipher }
    }

    fn encrypt_indexer(
        &self,
        indexer_id: &str,
        secrets: &BTreeMap<String, String>,
    ) -> Result<EncryptedSecrets, StorageError> {
        self.encrypt_scoped(&associated_indexer_data(indexer_id), secrets)
    }

    fn decrypt_indexer(
        &self,
        indexer_id: &str,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<BTreeMap<String, String>, StorageError> {
        self.decrypt_scoped(&associated_indexer_data(indexer_id), nonce, ciphertext)
    }

    pub(crate) fn encrypt_proxy_password(
        &self,
        proxy_id: &str,
        password: &str,
    ) -> Result<EncryptedSecrets, StorageError> {
        self.encrypt_scoped(
            &format!("indexloom:proxy-secrets:v{KEY_VERSION}:{proxy_id}"),
            &BTreeMap::from([("password".to_owned(), password.to_owned())]),
        )
    }

    pub(crate) fn decrypt_proxy_password(
        &self,
        proxy_id: &str,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<String, StorageError> {
        self.decrypt_scoped(
            &format!("indexloom:proxy-secrets:v{KEY_VERSION}:{proxy_id}"),
            nonce,
            ciphertext,
        )?
        .remove("password")
        .ok_or_else(|| StorageError::InvalidData("encrypted proxy password is missing".to_owned()))
    }

    fn encrypt_scoped(
        &self,
        associated_data: &str,
        secrets: &BTreeMap<String, String>,
    ) -> Result<EncryptedSecrets, StorageError> {
        let mut plaintext = serde_json::to_vec(secrets)?;
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| StorageError::RandomSource)?;
        let nonce_array = XNonce::from(nonce);
        let ciphertext = self.cipher.encrypt(
            &nonce_array,
            Payload {
                msg: &plaintext,
                aad: associated_data.as_bytes(),
            },
        );
        plaintext.zeroize();
        let ciphertext = ciphertext.map_err(|_| StorageError::Encryption)?;
        Ok(EncryptedSecrets {
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    fn decrypt_scoped(
        &self,
        associated_data: &str,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<BTreeMap<String, String>, StorageError> {
        if nonce.len() != NONCE_BYTES {
            return Err(StorageError::InvalidData(
                "encrypted credential nonce has an invalid length".to_owned(),
            ));
        }
        let nonce_array = XNonce::try_from(nonce).map_err(|_| {
            StorageError::InvalidData("encrypted credential nonce is invalid".to_owned())
        })?;
        let mut plaintext = self
            .cipher
            .decrypt(
                &nonce_array,
                Payload {
                    msg: ciphertext,
                    aad: associated_data.as_bytes(),
                },
            )
            .map_err(|_| StorageError::Decryption)?;
        let secrets = serde_json::from_slice(&plaintext).map_err(StorageError::from);
        plaintext.zeroize();
        secrets
    }
}

fn associated_indexer_data(indexer_id: &str) -> String {
    format!("indexloom:indexer-secrets:v{KEY_VERSION}:{indexer_id}")
}

pub(crate) struct EncryptedSecrets {
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

/// Stored indexer plus its definition and redacted configuration state.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerConfigurationRecord {
    /// Indexer instance ID.
    pub id: String,
    /// Definition used by the instance.
    pub definition_id: String,
    /// User-facing instance name.
    pub display_name: String,
    /// Whether searches may use this instance.
    pub enabled: bool,
    /// Whether the referenced definition still exists in the current catalog.
    pub catalog_available: bool,
    /// Optional per-indexer browser `User-Agent` override.
    pub user_agent: Option<String>,
    /// Optional `FlareSolverr` connector ID.
    pub flaresolverr_instance_id: Option<String>,
    /// Optional proxy explicitly assigned to this indexer.
    pub proxy_id: Option<String>,
    /// Group identifiers assigned to this indexer.
    pub group_ids: Vec<String>,
    /// Free-form normalized tags assigned to this indexer.
    pub tags: Vec<String>,
    /// Latest persisted connectivity test, if one has run.
    pub last_test: Option<LastIndexerTest>,
    /// Non-secret Cardigann configuration values.
    pub values: BTreeMap<String, Value>,
    /// Secret field names that currently contain a value.
    pub configured_secrets: Vec<String>,
    /// Definition YAML, omitted from serialization and used by the API layer.
    #[serde(skip)]
    pub definition_yaml: String,
}

/// Decrypted execution-only configuration that must never be serialized.
pub struct ResolvedIndexerConfiguration {
    /// Optional per-indexer browser `User-Agent` override.
    pub user_agent: Option<String>,
    /// Optional `FlareSolverr` connector ID.
    pub flaresolverr_instance_id: Option<String>,
    /// Optional proxy explicitly assigned to this indexer.
    pub proxy_id: Option<String>,
    /// Non-secret Cardigann values.
    pub values: BTreeMap<String, Value>,
    /// Decrypted tracker credentials.
    pub secrets: BTreeMap<String, String>,
}

impl fmt::Debug for ResolvedIndexerConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedIndexerConfiguration")
            .field("user_agent", &self.user_agent)
            .field("flaresolverr_instance_id", &self.flaresolverr_instance_id)
            .field("proxy_id", &self.proxy_id)
            .field("values", &self.values)
            .field("secret_names", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// One configured `FlareSolverr` service.
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlareSolverrInstance {
    /// Stable lowercase ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Base HTTP endpoint, normally ending in `/v1`.
    pub endpoint: String,
    /// Whether new requests may use this connector.
    pub enabled: bool,
    /// Maximum solver request duration.
    pub timeout_seconds: i64,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// RFC 3339 last-update time.
    pub updated_at: String,
}

/// Input used to create or replace a `FlareSolverr` connector.
#[derive(Clone, Debug)]
pub struct FlareSolverrInput {
    /// Stable lowercase ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Validated HTTP endpoint.
    pub endpoint: String,
    /// Whether the connector is enabled.
    pub enabled: bool,
    /// Solver timeout in seconds.
    pub timeout_seconds: i64,
}

#[derive(FromRow)]
struct IndexerRow {
    id: String,
    definition_id: String,
    display_name: String,
    enabled: bool,
    catalog_available: bool,
    configuration_json: String,
    user_agent: Option<String>,
    flaresolverr_instance_id: Option<String>,
    proxy_id: Option<String>,
    content_yaml: String,
    nonce: Option<Vec<u8>>,
    ciphertext: Option<Vec<u8>>,
}

impl Store {
    /// Opens the database, enables WAL for file databases and applies migrations.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the URL is invalid, SQLite cannot open the
    /// database, or a migration cannot be applied.
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let is_memory = database_url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if !is_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 5 })
            .connect_with(options)
            .await?;

        sqlx::migrate!("../../migrations/sqlite").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Confirms that the storage connection is usable.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the probe query cannot complete.
    pub async fn ping(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Lists indexers and redacted credential state.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query, JSON, or decryption failures.
    pub async fn list_indexers(
        &self,
        vault: &SecretVault,
    ) -> Result<Vec<IndexerConfigurationRecord>, StorageError> {
        let rows = sqlx::query_as::<_, IndexerRow>(
            "SELECT i.id, i.definition_id, i.display_name, i.enabled, \
                    i.configuration_json, i.user_agent, i.flaresolverr_instance_id, i.proxy_id, \
                    d.catalog_available, d.content_yaml, s.nonce, s.ciphertext \
             FROM indexers i \
             JOIN indexer_definitions d ON d.id = i.definition_id \
             LEFT JOIN indexer_secrets s ON s.indexer_id = i.id \
             ORDER BY i.display_name COLLATE NOCASE, i.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let metadata = self.load_indexer_metadata().await?;
        rows.into_iter()
            .map(|row| {
                let extra = metadata.get(&row.id).cloned().unwrap_or_default();
                row.redacted(vault, extra)
            })
            .collect()
    }

    /// Loads one indexer and its redacted credential state.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the indexer is missing or its persisted
    /// configuration cannot be parsed or decrypted.
    pub async fn get_indexer(
        &self,
        indexer_id: &str,
        vault: &SecretVault,
    ) -> Result<IndexerConfigurationRecord, StorageError> {
        let row = self.indexer_row(indexer_id).await?;
        let mut metadata = self.load_indexer_metadata().await?;
        let extra = metadata.remove(indexer_id).unwrap_or_default();
        row.redacted(vault, extra)
    }

    /// Loads one indexer with decrypted execution credentials.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the indexer is missing or data cannot be
    /// parsed or decrypted.
    pub async fn resolve_indexer_configuration(
        &self,
        indexer_id: &str,
        vault: &SecretVault,
    ) -> Result<ResolvedIndexerConfiguration, StorageError> {
        let row = self.indexer_row(indexer_id).await?;
        let values = parse_values(&row.configuration_json)?;
        let secrets = decrypt_row_secrets(&row, vault)?;
        Ok(ResolvedIndexerConfiguration {
            user_agent: row.user_agent,
            flaresolverr_instance_id: row.flaresolverr_instance_id,
            proxy_id: row.proxy_id,
            values,
            secrets,
        })
    }

    /// Replaces non-secret values and encrypted secret values atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for missing indexers, invalid references,
    /// serialization, encryption, or database failures.
    pub async fn save_indexer_configuration(
        &self,
        indexer_id: &str,
        user_agent: Option<&str>,
        flaresolverr_instance_id: Option<&str>,
        values: &BTreeMap<String, Value>,
        secrets: &BTreeMap<String, String>,
        vault: &SecretVault,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let now = now()?;
        let configuration_json = serde_json::to_string(values)?;
        let update = sqlx::query(
            "UPDATE indexers \
             SET configuration_json = ?, user_agent = ?, \
                 flaresolverr_instance_id = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(configuration_json)
        .bind(user_agent)
        .bind(flaresolverr_instance_id)
        .bind(&now)
        .bind(indexer_id)
        .execute(&mut *transaction)
        .await?;
        if update.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "indexer {indexer_id} was not found"
            )));
        }
        save_secrets(&mut transaction, indexer_id, secrets, vault, &now).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Lists configured `FlareSolverr` connectors.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when SQLite cannot execute the query.
    pub async fn list_flaresolverr(&self) -> Result<Vec<FlareSolverrInstance>, StorageError> {
        sqlx::query_as::<_, FlareSolverrInstance>(
            "SELECT id, name, endpoint, enabled, timeout_seconds, created_at, updated_at \
             FROM flaresolverr_instances ORDER BY name COLLATE NOCASE, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Creates or replaces one `FlareSolverr` connector.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when persistence fails.
    pub async fn upsert_flaresolverr(
        &self,
        input: &FlareSolverrInput,
    ) -> Result<FlareSolverrInstance, StorageError> {
        let now = now()?;
        sqlx::query(
            "INSERT INTO flaresolverr_instances \
                 (id, name, endpoint, enabled, timeout_seconds, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, endpoint = excluded.endpoint, \
                 enabled = excluded.enabled, timeout_seconds = excluded.timeout_seconds, \
                 updated_at = excluded.updated_at",
        )
        .bind(&input.id)
        .bind(&input.name)
        .bind(&input.endpoint)
        .bind(input.enabled)
        .bind(input.timeout_seconds)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        sqlx::query_as::<_, FlareSolverrInstance>(
            "SELECT id, name, endpoint, enabled, timeout_seconds, created_at, updated_at \
             FROM flaresolverr_instances WHERE id = ?",
        )
        .bind(&input.id)
        .fetch_one(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Deletes a `FlareSolverr` connector and clears indexer references.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when persistence fails or the connector is
    /// missing.
    pub async fn delete_flaresolverr(&self, id: &str) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM flaresolverr_instances WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "FlareSolverr connector {id} was not found"
            )));
        }
        Ok(())
    }

    /// Inserts or replaces a definition for catalog synchronization.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when persistence fails.
    pub async fn upsert_definition(
        &self,
        id: &str,
        source: &str,
        source_revision: Option<&str>,
        schema_version: &str,
        content_yaml: &str,
        content_sha256: &str,
    ) -> Result<(), StorageError> {
        let now = now()?;
        sqlx::query(
            "INSERT INTO indexer_definitions \
                 (id, source, source_revision, schema_version, content_yaml, content_sha256, imported_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                 source = excluded.source, source_revision = excluded.source_revision, \
                 schema_version = excluded.schema_version, content_yaml = excluded.content_yaml, \
                 content_sha256 = excluded.content_sha256, imported_at = excluded.imported_at, \
                 catalog_available = 1, catalog_generation = NULL",
        )
        .bind(id)
        .bind(source)
        .bind(source_revision)
        .bind(schema_version)
        .bind(content_yaml)
        .bind(content_sha256)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Creates an indexer instance from an installed definition.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the definition is missing or persistence
    /// fails.
    pub async fn create_indexer(
        &self,
        id: &str,
        definition_id: &str,
        display_name: &str,
        enabled: bool,
    ) -> Result<(), StorageError> {
        let now = now()?;
        sqlx::query(
            "INSERT INTO indexers \
                 (id, definition_id, display_name, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(definition_id)
        .bind(display_name)
        .bind(enabled)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn indexer_row(&self, indexer_id: &str) -> Result<IndexerRow, StorageError> {
        sqlx::query_as::<_, IndexerRow>(
            "SELECT i.id, i.definition_id, i.display_name, i.enabled, \
                    i.configuration_json, i.user_agent, i.flaresolverr_instance_id, i.proxy_id, \
                    d.catalog_available, d.content_yaml, s.nonce, s.ciphertext \
             FROM indexers i \
             JOIN indexer_definitions d ON d.id = i.definition_id \
             LEFT JOIN indexer_secrets s ON s.indexer_id = i.id \
             WHERE i.id = ?",
        )
        .bind(indexer_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StorageError::NotFound(format!("indexer {indexer_id} was not found")))
    }
}

impl IndexerRow {
    fn redacted(
        self,
        vault: &SecretVault,
        metadata: routing::IndexerMetadata,
    ) -> Result<IndexerConfigurationRecord, StorageError> {
        let values = parse_values(&self.configuration_json)?;
        let configured_secrets = decrypt_row_secrets(&self, vault)?.into_keys().collect();
        Ok(IndexerConfigurationRecord {
            id: self.id,
            definition_id: self.definition_id,
            display_name: self.display_name,
            enabled: self.enabled,
            catalog_available: self.catalog_available,
            user_agent: self.user_agent,
            flaresolverr_instance_id: self.flaresolverr_instance_id,
            proxy_id: self.proxy_id,
            group_ids: metadata.group_ids,
            tags: metadata.tags,
            last_test: metadata.last_test,
            values,
            configured_secrets,
            definition_yaml: self.content_yaml,
        })
    }
}

fn decrypt_row_secrets(
    row: &IndexerRow,
    vault: &SecretVault,
) -> Result<BTreeMap<String, String>, StorageError> {
    match (&row.nonce, &row.ciphertext) {
        (Some(nonce), Some(ciphertext)) => vault.decrypt_indexer(&row.id, nonce, ciphertext),
        (None, None) => Ok(BTreeMap::new()),
        _ => Err(StorageError::InvalidData(
            "encrypted credential row is incomplete".to_owned(),
        )),
    }
}

fn parse_values(configuration_json: &str) -> Result<BTreeMap<String, Value>, StorageError> {
    serde_json::from_str(configuration_json).map_err(StorageError::from)
}

async fn save_secrets(
    transaction: &mut Transaction<'_, Sqlite>,
    indexer_id: &str,
    secrets: &BTreeMap<String, String>,
    vault: &SecretVault,
    now: &str,
) -> Result<(), StorageError> {
    if secrets.is_empty() {
        sqlx::query("DELETE FROM indexer_secrets WHERE indexer_id = ?")
            .bind(indexer_id)
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    }
    let encrypted = vault.encrypt_indexer(indexer_id, secrets)?;
    sqlx::query(
        "INSERT INTO indexer_secrets \
             (indexer_id, key_version, nonce, ciphertext, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(indexer_id) DO UPDATE SET \
             key_version = excluded.key_version, nonce = excluded.nonce, \
             ciphertext = excluded.ciphertext, updated_at = excluded.updated_at",
    )
    .bind(indexer_id)
    .bind(KEY_VERSION)
    .bind(encrypted.nonce)
    .bind(encrypted.ciphertext)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) fn now() -> Result<String, StorageError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| StorageError::InvalidData(error.to_string()))
}

/// Storage initialization, crypto, and query failures.
#[derive(Debug, Error)]
pub enum StorageError {
    /// SQLite configuration or a query failed.
    #[error("SQLite failure: {0}")]
    Configuration(#[from] sqlx::Error),
    /// A database migration failed.
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// JSON serialization or parsing failed.
    #[error("JSON failure: {0}")]
    Json(#[from] serde_json::Error),
    /// The configured master key is not a 32-byte standard Base64 value.
    #[error("INDEXLOOM_MASTER_KEY must be standard Base64 encoding of exactly 32 bytes")]
    InvalidMasterKey,
    /// The operating system random source failed.
    #[error("operating system random source failed")]
    RandomSource,
    /// Authenticated encryption failed.
    #[error("credential encryption failed")]
    Encryption,
    /// Authenticated decryption failed because the key or data is invalid.
    #[error("credential decryption failed")]
    Decryption,
    /// Persisted data violates a storage invariant.
    #[error("invalid persisted data: {0}")]
    InvalidData(String),
    /// The requested record does not exist.
    #[error("not found: {0}")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use base64::Engine as _;
    use serde_json::json;

    use super::{
        BASE64_STANDARD, FlareSolverrInput, IndexerGroupInput, MASTER_KEY_BYTES,
        OutboundProxyInput, ProxyKind, ProxyPasswordUpdate, SecretVault, Store,
    };

    const DEFINITION: &str = r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata: { id: example, name: Example, description: Example, language: en-US }
provenance:
  generatedBy: indexloom-importer/v1
  source:
    project: jackett
    repository: https://github.com/Jackett/Jackett
    revision: "0000000000000000000000000000000000000000"
    path: src/Jackett.Common/Definitions/example.yml
    sourceId: example
    license: GPL-2.0-only
    fileSha256: "0000000000000000000000000000000000000000000000000000000000000000"
    contentSha256: "1111111111111111111111111111111111111111111111111111111111111111"
spec:
  protocol: torrent
  privacy: private
  baseUrls: [https://example.invalid/]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings: []
    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }
"#;

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn retiring_catalog_definitions_preserves_configuration_and_manual_entries() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("database should initialize");
        let vault = SecretVault::generate().expect("key generation should work");
        let example = super::CatalogDefinitionInput {
            id: "example".into(),
            display_name: "Example".into(),
            source: "jackett".into(),
            source_revision: Some("a".repeat(40)),
            schema_version: "indexloom.io/v1alpha1".into(),
            content_yaml: DEFINITION.into(),
            content_sha256: "a".repeat(64),
        };
        let other = super::CatalogDefinitionInput {
            id: "other".into(),
            display_name: "Other".into(),
            source: "prowlarr-indexers".into(),
            source_revision: Some("b".repeat(40)),
            schema_version: "indexloom.io/v1alpha1".into(),
            content_yaml: DEFINITION.replace("id: example", "id: other"),
            content_sha256: "b".repeat(64),
        };
        store
            .sync_catalog(&[example.clone(), other.clone()])
            .await
            .expect("initial snapshot should sync");
        store
            .create_indexer("configured", "example", "Customized Example", true)
            .await
            .expect("custom indexer should persist");
        store
            .upsert_definition(
                "local",
                "manual",
                None,
                "indexloom.io/v1alpha1",
                DEFINITION,
                &"f".repeat(64),
            )
            .await
            .expect("manual definition should persist");
        store
            .create_indexer("local-active", "local", "Manual", true)
            .await
            .expect("manual instance should persist");

        let initial = store
            .select_indexer_ids(true, &[], &[], true)
            .await
            .expect("initial selection");
        assert!(initial.contains(&"configured".to_owned()));
        assert!(initial.contains(&"local-active".to_owned()));

        let retired = store
            .sync_catalog(&[other])
            .await
            .expect("retiring an indexer should not delete it");
        assert!(retired.updated);
        let record = store
            .get_indexer("configured", &vault)
            .await
            .expect("retired instance must remain accessible");
        assert!(record.enabled, "user preference must be retained");
        assert!(
            !record.catalog_available,
            "missing definition must be marked unavailable"
        );
        for enabled_only in [false, true] {
            let selected = store
                .select_indexer_ids(true, &[], &[], enabled_only)
                .await
                .expect("selection after retirement");
            assert!(!selected.contains(&"configured".to_owned()));
            assert!(selected.contains(&"local-active".to_owned()));
        }
        assert!(
            store
                .get_indexer("local-active", &vault)
                .await
                .expect("manual record should survive")
                .catalog_available
        );

        let restored = store
            .sync_catalog(&[example])
            .await
            .expect("returning definition should reactivate availability");
        assert!(restored.updated);
        let record = store
            .get_indexer("configured", &vault)
            .await
            .expect("user settings should survive catalog restoration");
        assert!(record.catalog_available);
        assert!(record.enabled);
        assert!(
            store
                .select_indexer_ids(true, &[], &[], true)
                .await
                .expect("selection after restoration")
                .contains(&"configured".to_owned())
        );

        assert!(store.sync_catalog(&[]).await.is_err());
        assert!(
            store
                .get_indexer("configured", &vault)
                .await
                .expect("empty snapshot must not mutate persisted data")
                .catalog_available
        );
    }

    #[tokio::test]
    async fn encrypts_and_redacts_indexer_credentials() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("database should initialize");
        let vault = SecretVault::generate().expect("key generation should work");
        store
            .upsert_definition(
                "example",
                "test",
                None,
                "indexloom.io/v1alpha1",
                DEFINITION,
                &"0".repeat(64),
            )
            .await
            .expect("definition should persist");
        store
            .create_indexer("example-main", "example", "Example", true)
            .await
            .expect("indexer should persist");
        let connector = FlareSolverrInput {
            id: "local".to_owned(),
            name: "Local".to_owned(),
            endpoint: "http://flaresolverr:8191/v1".to_owned(),
            enabled: true,
            timeout_seconds: 60,
        };
        store
            .upsert_flaresolverr(&connector)
            .await
            .expect("connector should persist");

        let values = BTreeMap::from([("username".to_owned(), json!("alice"))]);
        let secrets = BTreeMap::from([
            ("password".to_owned(), "correct horse".to_owned()),
            ("cookie".to_owned(), "session=secret".to_owned()),
        ]);
        store
            .save_indexer_configuration(
                "example-main",
                Some("Mozilla/5.0"),
                Some("local"),
                &values,
                &secrets,
                &vault,
            )
            .await
            .expect("configuration should persist");

        let wrong_vault = SecretVault::generate().expect("second vault should initialize");
        assert!(store.list_indexers(&wrong_vault).await.is_err());

        let listed = store
            .list_indexers(&vault)
            .await
            .expect("indexers should load");
        assert_eq!(listed[0].configured_secrets, ["cookie", "password"]);
        let serialized = serde_json::to_string(&listed).expect("view should serialize");
        assert!(!serialized.contains("correct horse"));
        assert!(!serialized.contains("session=secret"));

        let resolved = store
            .resolve_indexer_configuration("example-main", &vault)
            .await
            .expect("credentials should decrypt");
        assert_eq!(resolved.secrets["password"], "correct horse");
        assert_eq!(resolved.values["username"], "alice");

        store
            .delete_flaresolverr("local")
            .await
            .expect("connector should delete");
        let resolved = store
            .resolve_indexer_configuration("example-main", &vault)
            .await
            .expect("configuration should remain readable");
        assert!(resolved.flaresolverr_instance_id.is_none());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn resolves_proxy_precedence_and_group_selection() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("database should initialize");
        let vault = SecretVault::generate().expect("key generation should work");
        store
            .upsert_definition(
                "example",
                "test",
                None,
                "indexloom.io/v1alpha1",
                DEFINITION,
                &"0".repeat(64),
            )
            .await
            .expect("definition should persist");
        store
            .create_indexer("example-main", "example", "Example", true)
            .await
            .expect("indexer should persist");

        for (id, port) in [("global", 8001), ("group", 8002), ("explicit", 8003)] {
            store
                .upsert_proxy(
                    &OutboundProxyInput {
                        id: id.to_owned(),
                        name: id.to_owned(),
                        kind: ProxyKind::Socks5,
                        host: "proxy.invalid".to_owned(),
                        port,
                        enabled: true,
                        username: Some("proxyuser".to_owned()),
                    },
                    ProxyPasswordUpdate::Replace(format!("secret-{id}")),
                    &vault,
                )
                .await
                .expect("proxy should persist");
        }
        store
            .set_global_proxy_id(Some("global"))
            .await
            .expect("global route should persist");
        store
            .upsert_group(&IndexerGroupInput {
                id: "anime".to_owned(),
                name: "Anime".to_owned(),
                description: String::new(),
                enabled: true,
                priority: 50,
                proxy_id: Some("group".to_owned()),
            })
            .await
            .expect("group should persist");
        store
            .save_indexer_organization(
                "example-main",
                "Example",
                true,
                None,
                &["anime".to_owned()],
                &["private".to_owned()],
            )
            .await
            .expect("organization should persist");

        let inherited = store
            .effective_proxy_for_indexer("example-main", &vault)
            .await
            .expect("effective proxy should resolve")
            .expect("group route should exist");
        assert_eq!(inherited.id, "group");
        assert_eq!(inherited.password.as_deref(), Some("secret-group"));
        assert_eq!(inherited.transport_url(), "socks5h://proxy.invalid:8002");

        store
            .save_indexer_organization(
                "example-main",
                "Example",
                true,
                Some("explicit"),
                &["anime".to_owned()],
                &["private".to_owned()],
            )
            .await
            .expect("explicit route should persist");
        let explicit = store
            .effective_proxy_for_indexer("example-main", &vault)
            .await
            .expect("effective proxy should resolve")
            .expect("explicit route should exist");
        assert_eq!(explicit.id, "explicit");

        let selected = store
            .select_indexer_ids(false, &[], &["anime".to_owned()], true)
            .await
            .expect("group selection should resolve");
        assert_eq!(selected, ["example-main"]);
        let listed = store
            .list_indexers(&vault)
            .await
            .expect("indexer should load");
        assert_eq!(listed[0].group_ids, ["anime"]);
        assert_eq!(listed[0].tags, ["private"]);
        let proxies = store
            .list_proxies(&vault)
            .await
            .expect("proxies should load");
        let serialized = serde_json::to_string(&proxies).expect("proxies should serialize");
        assert!(!serialized.contains("secret-"));
    }

    #[test]
    fn rejects_invalid_master_key_lengths() {
        assert!(SecretVault::from_base64("c2hvcnQ=").is_err());
    }

    #[test]
    fn rejects_master_keys_with_unprocessed_whitespace() {
        let encoded = BASE64_STANDARD.encode([42_u8; MASTER_KEY_BYTES]);
        assert!(SecretVault::from_base64(&format!(" {encoded}")).is_err());
        assert!(SecretVault::from_base64(&format!("{encoded}\n")).is_err());
    }
}
