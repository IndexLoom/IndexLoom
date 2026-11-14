//! Durable integration metadata; credentials are encrypted with identity-bound AEAD.
use crate::{SecretVault, StorageError, Store};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::BTreeMap;
use zeroize::Zeroize;

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Integration {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub origin: String,
    pub enabled: bool,
}

/// Intentionally has no Debug or Serialize implementation.
#[allow(
    missing_debug_implementations,
    reason = "Credentials must never be formatted for logs"
)]
#[derive(Default, Deserialize)]
#[serde(transparent)]
pub struct IntegrationSecrets(pub BTreeMap<String, String>);
impl Drop for IntegrationSecrets {
    fn drop(&mut self) {
        for value in self.0.values_mut() {
            value.zeroize();
        }
    }
}
type EncryptedRow = (Option<Vec<u8>>, Option<Vec<u8>>);

/// Persisted ownership of a named application provider.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct ManagedProviderName {
    pub local_indexer_id: String,
    pub provider_type: String,
    pub remote_name: String,
    pub remote_id: Option<i64>,
}

impl Store {
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn list_integrations(&self) -> Result<Vec<Integration>, StorageError> {
        Ok(
            sqlx::query_as("SELECT id,provider,name,origin,enabled FROM integrations ORDER BY id")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn get_integration(&self, id: &str) -> Result<Option<Integration>, StorageError> {
        Ok(
            sqlx::query_as("SELECT id,provider,name,origin,enabled FROM integrations WHERE id=?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    /// None preserves credentials; an empty map clears them. Identity/provider/origin changes
    /// require explicit credentials so old credentials cannot be sent to a new destination.
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn upsert_integration(
        &self,
        item: &Integration,
        secrets: Option<&IntegrationSecrets>,
        vault: &SecretVault,
    ) -> Result<(), StorageError> {
        let encrypted = secrets
            .map(|s| vault.encrypt_scoped(&scope(item), &s.0))
            .transpose()?;
        let mut tx = self.pool.begin().await?;
        let previous: Option<Integration> =
            sqlx::query_as("SELECT id,provider,name,origin,enabled FROM integrations WHERE id=?")
                .bind(&item.id)
                .fetch_optional(&mut *tx)
                .await?;
        if previous
            .as_ref()
            .is_some_and(|old| old.origin != item.origin || old.provider != item.provider)
        {
            return Err(StorageError::InvalidData(
                "integration destination and provider are immutable; delete and recreate".into(),
            ));
        }
        sqlx::query("INSERT INTO integrations(id,provider,name,origin,enabled) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,enabled=excluded.enabled,updated_at=CURRENT_TIMESTAMP")
            .bind(&item.id).bind(&item.provider).bind(&item.name).bind(&item.origin).bind(item.enabled).execute(&mut *tx).await?;
        if let Some(encrypted) = encrypted {
            sqlx::query("UPDATE integrations SET secret_nonce=?,secret_ciphertext=? WHERE id=?")
                .bind(encrypted.nonce)
                .bind(encrypted.ciphertext)
                .bind(&item.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn integration_secrets(
        &self,
        item: &Integration,
        vault: &SecretVault,
    ) -> Result<IntegrationSecrets, StorageError> {
        let row: Option<EncryptedRow> =
            sqlx::query_as("SELECT secret_nonce,secret_ciphertext FROM integrations WHERE id=?")
                .bind(&item.id)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((Some(nonce), Some(ciphertext))) => Ok(IntegrationSecrets(vault.decrypt_scoped(
                &scope(item),
                &nonce,
                &ciphertext,
            )?)),
            _ => Ok(IntegrationSecrets::default()),
        }
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn delete_integration(&self, id: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM integrations WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn integration_profile(&self, id: &str) -> Result<Option<String>, StorageError> {
        Ok(sqlx::query_scalar("SELECT profile_json FROM integration_sync_profiles WHERE integration_id=? AND id='default'").bind(id).fetch_optional(&self.pool).await?)
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn save_integration_profile(&self, id: &str, json: &str) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO integration_sync_profiles(id,integration_id,profile_json) VALUES('default',?,?) ON CONFLICT(integration_id,id) DO UPDATE SET profile_json=excluded.profile_json,updated_at=CURRENT_TIMESTAMP").bind(id).bind(json).execute(&self.pool).await?;
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn managed_indexers(&self, id: &str) -> Result<BTreeMap<String, i64>, StorageError> {
        Ok(sqlx::query_as::<_,(String,i64)>("SELECT local_indexer_id,remote_indexer_id FROM integration_managed_indexers WHERE integration_id=? ORDER BY local_indexer_id").bind(id).fetch_all(&self.pool).await?.into_iter().collect())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn set_managed_indexer(
        &self,
        id: &str,
        local: &str,
        remote: i64,
    ) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO integration_managed_indexers(integration_id,local_indexer_id,remote_indexer_id) VALUES(?,?,?) ON CONFLICT(integration_id,local_indexer_id) DO UPDATE SET remote_indexer_id=excluded.remote_indexer_id").bind(id).bind(local).bind(remote).execute(&self.pool).await?;
        Ok(())
    }
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn forget_managed_indexer(&self, id: &str, local: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM integration_managed_indexers WHERE integration_id=? AND local_indexer_id=?").bind(id).bind(local).execute(&self.pool).await?;
        Ok(())
    }
    /// # Errors
    /// Returns a database error if ownership cannot be loaded.
    pub async fn managed_provider_names(
        &self,
        id: &str,
    ) -> Result<Vec<ManagedProviderName>, StorageError> {
        Ok(sqlx::query_as("SELECT local_indexer_id,provider_type,remote_name,remote_id FROM integration_managed_provider_names WHERE integration_id=? ORDER BY local_indexer_id").bind(id).fetch_all(&self.pool).await?)
    }
    /// # Errors
    /// Returns a database error for conflicting ownership or invalid provider data.
    pub async fn set_managed_provider_name(
        &self,
        id: &str,
        mapping: &ManagedProviderName,
    ) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO integration_managed_provider_names(integration_id,local_indexer_id,provider_type,remote_name,remote_id) VALUES(?,?,?,?,?) ON CONFLICT(integration_id,local_indexer_id) DO UPDATE SET provider_type=excluded.provider_type,remote_name=excluded.remote_name,remote_id=excluded.remote_id")
            .bind(id).bind(&mapping.local_indexer_id).bind(&mapping.provider_type).bind(&mapping.remote_name).bind(mapping.remote_id).execute(&self.pool).await?;
        Ok(())
    }
    /// # Errors
    /// Returns a database error if ownership cannot be removed.
    pub async fn forget_managed_provider_name(
        &self,
        id: &str,
        local: &str,
    ) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM integration_managed_provider_names WHERE integration_id=? AND local_indexer_id=?").bind(id).bind(local).execute(&self.pool).await?;
        Ok(())
    }
    /// Event values are fixed codes; never persist upstream response bodies or credentials.
    /// # Errors
    /// Returns a storage error for database failures or invalid saved data.
    pub async fn integration_event(
        &self,
        id: &str,
        operation: &str,
        success: bool,
    ) -> Result<(), StorageError> {
        if !matches!(operation, "sync" | "test") {
            return Err(StorageError::InvalidData(
                "invalid integration event".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO integration_events(integration_id,operation,outcome) VALUES(?,?,?)",
        )
        .bind(id)
        .bind(operation)
        .bind(if success { "success" } else { "failure" })
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
fn scope(item: &Integration) -> String {
    format!(
        "indexloom:integration-secrets:v1:{}:{}:{}",
        item.id, item.provider, item.origin
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn named_ownership_unique_and_cascading() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let vault = SecretVault::generate().unwrap();
        let app = Integration {
            id: "named".into(),
            provider: "mylar".into(),
            name: "Named".into(),
            origin: "http://localhost:8090".into(),
            enabled: true,
        };
        store.upsert_integration(&app, None, &vault).await.unwrap();
        let mut m = ManagedProviderName {
            local_indexer_id: "local".into(),
            provider_type: "torznab".into(),
            remote_name: "IndexLoom [named/local]".into(),
            remote_id: Some(7),
        };
        store.set_managed_provider_name("named", &m).await.unwrap();
        assert_eq!(
            store.managed_provider_names("named").await.unwrap(),
            vec![m.clone()]
        );
        m.local_indexer_id = "other".into();
        assert!(store.set_managed_provider_name("named", &m).await.is_err());
        m.remote_name = "IndexLoom [named/other]".into();
        assert!(store.set_managed_provider_name("named", &m).await.is_err());
        m.provider_type = "newznab".into();
        store.set_managed_provider_name("named", &m).await.unwrap();
        m.remote_id = None;
        store.set_managed_provider_name("named", &m).await.unwrap();
        assert!(store.managed_indexers("named").await.unwrap().is_empty());
        store.delete_integration("named").await.unwrap();
        assert!(
            store
                .managed_provider_names("named")
                .await
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn encrypted_crud_and_identity_binding() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let vault = SecretVault::generate().unwrap();
        let mut item = Integration {
            id: "app".into(),
            provider: "sonarr".into(),
            name: "App".into(),
            origin: "http://localhost:8989".into(),
            enabled: true,
        };
        let secrets =
            IntegrationSecrets(BTreeMap::from([("apiKey".into(), "never-public".into())]));
        store
            .upsert_integration(&item, Some(&secrets), &vault)
            .await
            .unwrap();
        assert!(
            !serde_json::to_string(&store.list_integrations().await.unwrap())
                .unwrap()
                .contains("never-public")
        );
        let cipher: Vec<u8> = sqlx::query_scalar("SELECT secret_ciphertext FROM integrations")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(!cipher.windows(12).any(|w| w == b"never-public"));
        assert_eq!(
            store.integration_secrets(&item, &vault).await.unwrap().0["apiKey"],
            "never-public"
        );
        store.save_integration_profile("app", "{}").await.unwrap();
        store
            .save_integration_profile("app", "{\"updated\":true}")
            .await
            .unwrap();
        item.origin = "http://localhost:9999".into();
        assert!(store.integration_secrets(&item, &vault).await.is_err());
        assert!(store.upsert_integration(&item, None, &vault).await.is_err());
        store.delete_integration("app").await.unwrap();
        assert!(store.integration_profile("app").await.unwrap().is_none());
    }
}
