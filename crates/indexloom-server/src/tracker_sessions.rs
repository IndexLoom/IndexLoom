//! Adapts runtime snapshots to encrypted storage. No session API routes.

use indexloom_engine::{CookieSnapshot, EngineError, SessionPersistence};
use indexloom_storage::{SecretVault, Store, TrackerSession};

#[derive(Debug)]
pub(crate) struct StoredTrackerSession {
    pub store: Store,
    pub vault: SecretVault,
    pub indexer_id: String,
    pub revision: i64,
}

#[async_trait::async_trait]
impl SessionPersistence for StoredTrackerSession {
    async fn load(&self) -> Result<Option<CookieSnapshot>, EngineError> {
        self.store
            .load_tracker_session(&self.indexer_id, self.revision, &self.vault)
            .await
            .map(|session| {
                session.map(|session| CookieSnapshot {
                    cookies_json: session.cookies_json,
                    expires_at: session.expires_at,
                })
            })
            .map_err(|_| EngineError::SessionPersistence)
    }

    async fn save(&self, snapshot: CookieSnapshot) -> Result<(), EngineError> {
        let current = self
            .store
            .save_tracker_session(
                &self.indexer_id,
                self.revision,
                &TrackerSession {
                    cookies_json: snapshot.cookies_json,
                    expires_at: snapshot.expires_at,
                },
                &self.vault,
            )
            .await
            .map_err(|_| EngineError::SessionPersistence)?;
        if current {
            Ok(())
        } else {
            Err(EngineError::SessionPersistence)
        }
    }

    async fn clear(&self) -> Result<(), EngineError> {
        self.store
            .clear_tracker_session(&self.indexer_id, self.revision)
            .await
            .map_err(|_| EngineError::SessionPersistence)
    }
}
