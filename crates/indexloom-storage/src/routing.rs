use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Sqlite, Transaction};
use uuid::Uuid;

use super::{KEY_VERSION, SecretVault, StorageError, Store, now};

/// Supported outbound proxy protocols.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyKind {
    /// Standard HTTP CONNECT/forward proxy.
    Http,
    /// `SOCKS4a` proxy with remote hostname resolution.
    Socks4,
    /// `SOCKS5h` proxy with remote hostname resolution.
    Socks5,
}

impl ProxyKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks4 => "socks4",
            Self::Socks5 => "socks5",
        }
    }

    /// URL scheme used by the HTTP client. The `a`/`h` variants deliberately
    /// resolve tracker hostnames through the proxy rather than leaking DNS.
    #[must_use]
    pub const fn transport_scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks4 => "socks4a",
            Self::Socks5 => "socks5h",
        }
    }
}

impl FromStr for ProxyKind {
    type Err = StorageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "http" => Ok(Self::Http),
            "socks4" => Ok(Self::Socks4),
            "socks5" => Ok(Self::Socks5),
            _ => Err(StorageError::InvalidData(format!(
                "unsupported outbound proxy kind {value}"
            ))),
        }
    }
}

/// Redacted outbound proxy returned by the management API.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundProxy {
    pub id: String,
    pub name: String,
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub enabled: bool,
    pub username: Option<String>,
    pub password_configured: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Validated proxy input.
#[derive(Clone, Debug)]
pub struct OutboundProxyInput {
    pub id: String,
    pub name: String,
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub enabled: bool,
    pub username: Option<String>,
}

/// Three-state password update used to avoid accidental credential erasure.
#[derive(Clone, Debug)]
pub enum ProxyPasswordUpdate {
    Preserve,
    Replace(String),
    Clear,
}

/// Execution-only proxy credentials. Debug output never includes the password.
#[derive(Clone)]
pub struct ResolvedOutboundProxy {
    pub id: String,
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub enabled: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ResolvedOutboundProxy {
    #[must_use]
    pub fn transport_url(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{}://{host}:{}", self.kind.transport_scheme(), self.port)
    }
}

impl fmt::Debug for ResolvedOutboundProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedOutboundProxy")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("enabled", &self.enabled)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Named, many-to-many indexer group.
#[derive(Clone, Debug, Deserialize, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerGroup {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub priority: i64,
    pub proxy_id: Option<String>,
    pub member_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Validated group input.
#[derive(Clone, Debug)]
pub struct IndexerGroupInput {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub priority: i64,
    pub proxy_id: Option<String>,
}

/// Latest persisted indexer test state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastIndexerTest {
    pub status: String,
    pub response_time_ms: Option<i64>,
    pub message: String,
    pub tested_at: String,
}

/// One test result to persist.
#[derive(Clone, Debug)]
pub struct IndexerTestRecord {
    pub indexer_id: String,
    pub success: bool,
    pub response_time_ms: Option<u64>,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct IndexerMetadata {
    pub group_ids: Vec<String>,
    pub tags: Vec<String>,
    pub last_test: Option<LastIndexerTest>,
}

#[derive(FromRow)]
struct ProxyRow {
    id: String,
    name: String,
    kind: String,
    host: String,
    port: i64,
    enabled: bool,
    username: Option<String>,
    created_at: String,
    updated_at: String,
    nonce: Option<Vec<u8>>,
    ciphertext: Option<Vec<u8>>,
}

impl ProxyRow {
    fn password(&self, vault: &SecretVault) -> Result<Option<String>, StorageError> {
        match (&self.nonce, &self.ciphertext) {
            (Some(nonce), Some(ciphertext)) => vault
                .decrypt_proxy_password(&self.id, nonce, ciphertext)
                .map(Some),
            (None, None) => Ok(None),
            _ => Err(StorageError::InvalidData(
                "encrypted proxy password row is incomplete".to_owned(),
            )),
        }
    }

    fn redacted(&self, vault: &SecretVault) -> Result<OutboundProxy, StorageError> {
        let password_configured = self.password(vault)?.is_some();
        Ok(OutboundProxy {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind.parse()?,
            host: self.host.clone(),
            port: u16::try_from(self.port).map_err(|_| {
                StorageError::InvalidData("persisted proxy port is invalid".to_owned())
            })?,
            enabled: self.enabled,
            username: self.username.clone(),
            password_configured,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        })
    }

    fn resolved(self, vault: &SecretVault) -> Result<ResolvedOutboundProxy, StorageError> {
        let password = self.password(vault)?;
        Ok(ResolvedOutboundProxy {
            id: self.id,
            kind: self.kind.parse()?,
            host: self.host,
            port: u16::try_from(self.port).map_err(|_| {
                StorageError::InvalidData("persisted proxy port is invalid".to_owned())
            })?,
            enabled: self.enabled,
            username: self.username,
            password,
        })
    }
}

impl Store {
    /// Lists redacted HTTP/SOCKS proxy connectors.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when rows cannot be loaded or decrypted.
    pub async fn list_proxies(
        &self,
        vault: &SecretVault,
    ) -> Result<Vec<OutboundProxy>, StorageError> {
        let rows = self.proxy_rows(None).await?;
        rows.iter().map(|row| row.redacted(vault)).collect()
    }

    /// Creates or updates a proxy while preserving, replacing, or clearing its password.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when encryption, validation, or persistence fails.
    pub async fn upsert_proxy(
        &self,
        input: &OutboundProxyInput,
        password_update: ProxyPasswordUpdate,
        vault: &SecretVault,
    ) -> Result<OutboundProxy, StorageError> {
        let mut transaction = self.pool.begin().await?;
        let timestamp = now()?;
        sqlx::query(
            "INSERT INTO outbound_proxies \
                 (id, name, kind, host, port, enabled, username, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, kind = excluded.kind, \
                 host = excluded.host, port = excluded.port, enabled = excluded.enabled, \
                 username = excluded.username, updated_at = excluded.updated_at",
        )
        .bind(&input.id)
        .bind(&input.name)
        .bind(input.kind.as_str())
        .bind(&input.host)
        .bind(i64::from(input.port))
        .bind(input.enabled)
        .bind(&input.username)
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&mut *transaction)
        .await?;

        match password_update {
            ProxyPasswordUpdate::Preserve => {}
            ProxyPasswordUpdate::Clear => {
                sqlx::query("DELETE FROM proxy_secrets WHERE proxy_id = ?")
                    .bind(&input.id)
                    .execute(&mut *transaction)
                    .await?;
            }
            ProxyPasswordUpdate::Replace(password) => {
                let encrypted = vault.encrypt_proxy_password(&input.id, &password)?;
                sqlx::query(
                    "INSERT INTO proxy_secrets \
                         (proxy_id, key_version, nonce, ciphertext, updated_at) \
                     VALUES (?, ?, ?, ?, ?) \
                     ON CONFLICT(proxy_id) DO UPDATE SET key_version = excluded.key_version, \
                         nonce = excluded.nonce, ciphertext = excluded.ciphertext, \
                         updated_at = excluded.updated_at",
                )
                .bind(&input.id)
                .bind(KEY_VERSION)
                .bind(encrypted.nonce)
                .bind(encrypted.ciphertext)
                .bind(&timestamp)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        self.proxy_rows(Some(&input.id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StorageError::NotFound(format!("proxy {} was not found", input.id)))?
            .redacted(vault)
    }

    /// Deletes one proxy and clears references through foreign-key actions.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the proxy is missing or SQLite fails.
    pub async fn delete_proxy(&self, id: &str) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM outbound_proxies WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!("proxy {id} was not found")));
        }
        Ok(())
    }

    /// Loads one execution-only proxy including decrypted credentials.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the proxy is missing or cannot be decrypted.
    pub async fn resolve_proxy(
        &self,
        id: &str,
        vault: &SecretVault,
    ) -> Result<ResolvedOutboundProxy, StorageError> {
        self.proxy_rows(Some(id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StorageError::NotFound(format!("proxy {id} was not found")))?
            .resolved(vault)
    }

    /// Resolves indexer > highest-priority enabled group > global proxy precedence.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the indexer is missing or routing data cannot be loaded.
    pub async fn effective_proxy_for_indexer(
        &self,
        indexer_id: &str,
        vault: &SecretVault,
    ) -> Result<Option<ResolvedOutboundProxy>, StorageError> {
        let explicit =
            sqlx::query_scalar::<_, Option<String>>("SELECT proxy_id FROM indexers WHERE id = ?")
                .bind(indexer_id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or_else(|| {
                    StorageError::NotFound(format!("indexer {indexer_id} was not found"))
                })?;
        let proxy_id = if explicit.is_some() {
            explicit
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT g.proxy_id FROM indexer_groups g \
                 JOIN indexer_group_members m ON m.group_id = g.id \
                 WHERE m.indexer_id = ? AND g.enabled = 1 AND g.proxy_id IS NOT NULL \
                 ORDER BY g.priority DESC, g.id ASC LIMIT 1",
            )
            .bind(indexer_id)
            .fetch_optional(&self.pool)
            .await?
            .or(sqlx::query_scalar::<_, String>(
                "SELECT global_proxy_id FROM runtime_settings \
                     WHERE singleton = 1 AND global_proxy_id IS NOT NULL",
            )
            .fetch_optional(&self.pool)
            .await?)
        };
        match proxy_id {
            Some(id) => self.resolve_proxy(&id, vault).await.map(Some),
            None => Ok(None),
        }
    }

    /// Returns the configured global proxy override.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when SQLite cannot load the singleton setting.
    pub async fn global_proxy_id(&self) -> Result<Option<String>, StorageError> {
        sqlx::query_scalar("SELECT global_proxy_id FROM runtime_settings WHERE singleton = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(StorageError::from)
    }

    /// Replaces the configured global proxy override.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the referenced proxy is invalid or SQLite fails.
    pub async fn set_global_proxy_id(&self, proxy_id: Option<&str>) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE runtime_settings SET global_proxy_id = ?, updated_at = ? WHERE singleton = 1",
        )
        .bind(proxy_id)
        .bind(now()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists groups with current member counts.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when SQLite cannot execute the query.
    pub async fn list_groups(&self) -> Result<Vec<IndexerGroup>, StorageError> {
        sqlx::query_as(
            "SELECT g.id, g.name, g.description, g.enabled, g.priority, g.proxy_id, \
                    COUNT(m.indexer_id) AS member_count, g.created_at, g.updated_at \
             FROM indexer_groups g \
             LEFT JOIN indexer_group_members m ON m.group_id = g.id \
             GROUP BY g.id ORDER BY g.priority DESC, g.name COLLATE NOCASE, g.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StorageError::from)
    }

    /// Creates or updates one named group.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when validation or persistence fails.
    pub async fn upsert_group(
        &self,
        input: &IndexerGroupInput,
    ) -> Result<IndexerGroup, StorageError> {
        let timestamp = now()?;
        sqlx::query(
            "INSERT INTO indexer_groups \
                 (id, name, description, enabled, priority, proxy_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, \
                 description = excluded.description, enabled = excluded.enabled, \
                 priority = excluded.priority, proxy_id = excluded.proxy_id, \
                 updated_at = excluded.updated_at",
        )
        .bind(&input.id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(input.enabled)
        .bind(input.priority)
        .bind(&input.proxy_id)
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&self.pool)
        .await?;
        self.list_groups()
            .await?
            .into_iter()
            .find(|group| group.id == input.id)
            .ok_or_else(|| StorageError::NotFound(format!("group {} was not found", input.id)))
    }

    /// Deletes a group without deleting its member indexers.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the group is missing or SQLite fails.
    pub async fn delete_group(&self, id: &str) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM indexer_groups WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!("group {id} was not found")));
        }
        Ok(())
    }

    /// Atomically changes instance state, proxy override, groups, and tags.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the indexer or references are missing or the transaction fails.
    pub async fn save_indexer_organization(
        &self,
        indexer_id: &str,
        display_name: &str,
        enabled: bool,
        proxy_id: Option<&str>,
        group_ids: &[String],
        tags: &[String],
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE indexers SET display_name = ?, enabled = ?, proxy_id = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(display_name)
        .bind(enabled)
        .bind(proxy_id)
        .bind(now()?)
        .bind(indexer_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "indexer {indexer_id} was not found"
            )));
        }
        replace_indexer_links(&mut transaction, indexer_id, group_ids, tags).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Resolves an all/explicit/group union into a stable, deduplicated list.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the indexer or membership rows cannot be loaded.
    pub async fn select_indexer_ids(
        &self,
        all: bool,
        indexer_ids: &[String],
        group_ids: &[String],
        enabled_only: bool,
    ) -> Result<Vec<String>, StorageError> {
        let available = sqlx::query_as::<_, IdEnabledRow>("SELECT id, enabled FROM indexers")
            .fetch_all(&self.pool)
            .await?;
        let enabled = available
            .iter()
            .filter(|row| !enabled_only || row.enabled)
            .map(|row| row.id.clone())
            .collect::<BTreeSet<_>>();
        if all {
            return Ok(enabled.into_iter().collect());
        }
        let mut selected = indexer_ids
            .iter()
            .filter(|id| enabled.contains(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        if !group_ids.is_empty() {
            let wanted = group_ids.iter().collect::<BTreeSet<_>>();
            let memberships = sqlx::query_as::<_, MembershipRow>(
                "SELECT m.group_id, m.indexer_id FROM indexer_group_members m \
                 JOIN indexer_groups g ON g.id = m.group_id WHERE g.enabled = 1",
            )
            .fetch_all(&self.pool)
            .await?;
            for membership in memberships {
                if wanted.contains(&membership.group_id) && enabled.contains(&membership.indexer_id)
                {
                    selected.insert(membership.indexer_id);
                }
            }
        }
        Ok(selected.into_iter().collect())
    }

    /// Persists a batch of indexer test outcomes atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when an indexer is missing or the transaction fails.
    pub async fn record_indexer_tests(
        &self,
        records: &[IndexerTestRecord],
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        for record in records {
            sqlx::query(
                "INSERT INTO indexer_test_runs \
                     (id, indexer_id, status, response_time_ms, message, tested_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(&record.indexer_id)
            .bind(if record.success { "success" } else { "failed" })
            .bind(
                record
                    .response_time_ms
                    .and_then(|value| i64::try_from(value).ok()),
            )
            .bind(&record.message)
            .bind(now()?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn load_indexer_metadata(
        &self,
    ) -> Result<BTreeMap<String, IndexerMetadata>, StorageError> {
        let mut metadata = BTreeMap::<String, IndexerMetadata>::new();
        for row in sqlx::query_as::<_, MembershipRow>(
            "SELECT group_id, indexer_id FROM indexer_group_members ORDER BY group_id",
        )
        .fetch_all(&self.pool)
        .await?
        {
            metadata
                .entry(row.indexer_id)
                .or_default()
                .group_ids
                .push(row.group_id);
        }
        for row in sqlx::query_as::<_, TagRow>(
            "SELECT indexer_id, tag FROM indexer_tags ORDER BY tag COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?
        {
            metadata
                .entry(row.indexer_id)
                .or_default()
                .tags
                .push(row.tag);
        }
        for row in sqlx::query_as::<_, LastTestRow>(
            "SELECT indexer_id, status, response_time_ms, message, tested_at \
             FROM indexer_test_runs ORDER BY tested_at DESC, id DESC",
        )
        .fetch_all(&self.pool)
        .await?
        {
            let entry = metadata.entry(row.indexer_id).or_default();
            if entry.last_test.is_none() {
                entry.last_test = Some(LastIndexerTest {
                    status: row.status,
                    response_time_ms: row.response_time_ms,
                    message: row.message,
                    tested_at: row.tested_at,
                });
            }
        }
        Ok(metadata)
    }

    async fn proxy_rows(&self, id: Option<&str>) -> Result<Vec<ProxyRow>, StorageError> {
        if let Some(id) = id {
            sqlx::query_as::<_, ProxyRow>(
                "SELECT p.id, p.name, p.kind, p.host, p.port, p.enabled, p.username, \
                        p.created_at, p.updated_at, s.nonce, s.ciphertext \
                 FROM outbound_proxies p \
                 LEFT JOIN proxy_secrets s ON s.proxy_id = p.id \
                 WHERE p.id = ?",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(StorageError::from)
        } else {
            sqlx::query_as::<_, ProxyRow>(
                "SELECT p.id, p.name, p.kind, p.host, p.port, p.enabled, p.username, \
                        p.created_at, p.updated_at, s.nonce, s.ciphertext \
                 FROM outbound_proxies p \
                 LEFT JOIN proxy_secrets s ON s.proxy_id = p.id \
                 ORDER BY p.name COLLATE NOCASE, p.id",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(StorageError::from)
        }
    }
}

#[derive(FromRow)]
struct IdEnabledRow {
    id: String,
    enabled: bool,
}

#[derive(FromRow)]
struct MembershipRow {
    group_id: String,
    indexer_id: String,
}

#[derive(FromRow)]
struct TagRow {
    indexer_id: String,
    tag: String,
}

#[derive(FromRow)]
struct LastTestRow {
    indexer_id: String,
    status: String,
    response_time_ms: Option<i64>,
    message: String,
    tested_at: String,
}

async fn replace_indexer_links(
    transaction: &mut Transaction<'_, Sqlite>,
    indexer_id: &str,
    group_ids: &[String],
    tags: &[String],
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM indexer_group_members WHERE indexer_id = ?")
        .bind(indexer_id)
        .execute(&mut **transaction)
        .await?;
    for group_id in group_ids {
        sqlx::query("INSERT INTO indexer_group_members (group_id, indexer_id) VALUES (?, ?)")
            .bind(group_id)
            .bind(indexer_id)
            .execute(&mut **transaction)
            .await?;
    }
    sqlx::query("DELETE FROM indexer_tags WHERE indexer_id = ?")
        .bind(indexer_id)
        .execute(&mut **transaction)
        .await?;
    for tag in tags {
        sqlx::query("INSERT INTO indexer_tags (indexer_id, tag) VALUES (?, ?)")
            .bind(indexer_id)
            .bind(tag)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}
