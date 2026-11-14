use std::collections::BTreeMap;

use super::*;

async fn fixture(store: &Store) {
    store
        .upsert_definition("fixture", "test", None, "v1", "fixture", &"0".repeat(64))
        .await
        .unwrap();
    for id in ["alice", "bob"] {
        store.create_indexer(id, "fixture", id, true).await.unwrap();
    }
}

fn snapshot(value: &str) -> TrackerSession {
    TrackerSession {
        cookies_json: value.into(),
        expires_at: OffsetDateTime::now_utc().unix_timestamp() + 3600,
    }
}

#[tokio::test]
async fn encrypted_isolated_redacted_and_durable_after_database_reopen() {
    let path = std::env::temp_dir().join(format!("indexloom-session-{}.db", uuid::Uuid::now_v7()));
    let vault = SecretVault::generate().unwrap();
    let store = Store::connect_path(&path).await.unwrap();
    fixture(&store).await;
    let session = snapshot("fixture-session-token");
    assert!(
        store
            .save_tracker_session("alice", 0, &session, &vault)
            .await
            .unwrap()
    );
    assert!(
        store
            .load_tracker_session("bob", 0, &vault)
            .await
            .unwrap()
            .is_none()
    );
    let ciphertext: Vec<u8> = sqlx::query_scalar("SELECT ciphertext FROM indexer_sessions")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert!(
        !ciphertext
            .windows(session.cookies_json.len())
            .any(|bytes| bytes == session.cookies_json.as_bytes())
    );
    assert!(!format!("{session:?}").contains("fixture-session-token"));
    let response = serde_json::to_string(&store.list_indexers(&vault).await.unwrap()).unwrap();
    assert!(!response.contains("fixture-session-token"));
    store.pool.close().await;
    let store = Store::connect_path(&path).await.unwrap();
    assert_eq!(
        store
            .load_tracker_session("alice", 0, &vault)
            .await
            .unwrap()
            .unwrap()
            .cookies_json,
        "fixture-session-token"
    );
    // The raw database and WAL must never contain cookie plaintext.
    for file in [path.clone(), path.with_extension("db-wal")] {
        if let Ok(bytes) = std::fs::read(file) {
            assert!(
                !bytes
                    .windows(21)
                    .any(|bytes| bytes == b"fixture-session-token")
            );
        }
    }
    store.pool.close().await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn authenticates_instance_revision_expiry_ciphertext_and_master_key() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    fixture(&store).await;
    let session = snapshot("token");
    store
        .save_tracker_session("alice", 0, &session, &vault)
        .await
        .unwrap();
    assert!(
        store
            .load_tracker_session("alice", 0, &SecretVault::generate().unwrap())
            .await
            .is_err()
    );
    sqlx::query("UPDATE indexer_sessions SET indexer_id = 'bob'")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(store.load_tracker_session("bob", 0, &vault).await.is_err());
    sqlx::query("UPDATE indexer_sessions SET indexer_id = 'alice', expires_at = expires_at + 1")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_tracker_session("alice", 0, &vault)
            .await
            .is_err()
    );
    sqlx::query("UPDATE indexer_sessions SET expires_at = expires_at - 1, revision = 1")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE indexers SET session_revision = 1 WHERE id = 'alice'")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_tracker_session("alice", 1, &vault)
            .await
            .is_err()
    );
    store
        .save_tracker_session("alice", 1, &session, &vault)
        .await
        .unwrap();
    sqlx::query("UPDATE indexer_sessions SET ciphertext = zeroblob(length(ciphertext))")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_tracker_session("alice", 1, &vault)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn configuration_change_revokes_and_fences_old_requests() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    fixture(&store).await;
    let session = snapshot("old-token");
    store
        .save_tracker_session("alice", 0, &session, &vault)
        .await
        .unwrap();
    store
        .save_indexer_configuration(
            "alice",
            None,
            None,
            &BTreeMap::new(),
            &BTreeMap::from([("password".into(), "fixture-password".into())]),
            &vault,
        )
        .await
        .unwrap();
    let config = store
        .resolve_indexer_configuration("alice", &vault)
        .await
        .unwrap();
    assert_eq!(config.session_revision, 1);
    assert!(
        store
            .load_tracker_session("alice", 0, &vault)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .save_tracker_session("alice", 0, &session, &vault)
            .await
            .unwrap()
    );
    assert!(
        store
            .save_tracker_session("alice", 1, &snapshot("new-token"), &vault)
            .await
            .unwrap()
    );
    store.clear_tracker_session("alice", 0).await.unwrap();
    assert_eq!(
        store
            .load_tracker_session("alice", 1, &vault)
            .await
            .unwrap()
            .unwrap()
            .cookies_json,
        "new-token"
    );
}

#[tokio::test]
async fn expiry_eviction_deletion_and_limits() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    fixture(&store).await;
    let session = snapshot("token");
    store
        .save_tracker_session("alice", 0, &session, &vault)
        .await
        .unwrap();
    sqlx::query("UPDATE indexer_sessions SET expires_at = 0")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_tracker_session("alice", 0, &vault)
            .await
            .unwrap()
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM indexer_sessions")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        store
            .save_tracker_session(
                "alice",
                0,
                &snapshot(&"x".repeat(MAX_SESSION_BYTES + 1)),
                &vault
            )
            .await
            .is_err()
    );
    assert!(
        store
            .save_tracker_session(
                "alice",
                0,
                &TrackerSession {
                    cookies_json: "[]".into(),
                    expires_at: OffsetDateTime::now_utc().unix_timestamp() + 13 * 86400
                },
                &vault
            )
            .await
            .is_err()
    );
    store
        .save_tracker_session("alice", 0, &session, &vault)
        .await
        .unwrap();
    sqlx::query("DELETE FROM indexers WHERE id = 'alice'")
        .execute(&store.pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM indexer_sessions")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn concurrent_sqlite_sessions_do_not_overwrite_other_instances() {
    let path =
        std::env::temp_dir().join(format!("indexloom-concurrent-{}.db", uuid::Uuid::now_v7()));
    let store = Store::connect_path(&path).await.unwrap();
    let vault = SecretVault::generate().unwrap();
    fixture(&store).await;
    let mut tasks = tokio::task::JoinSet::new();
    for id in ["alice", "bob"] {
        let (store, vault) = (store.clone(), vault.clone());
        tasks.spawn(async move {
            for _ in 0..20 {
                store
                    .save_tracker_session(id, 0, &snapshot(id), &vault)
                    .await
                    .unwrap();
            }
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap();
    }
    for id in ["alice", "bob"] {
        assert_eq!(
            store
                .load_tracker_session(id, 0, &vault)
                .await
                .unwrap()
                .unwrap()
                .cookies_json,
            id
        );
    }
    store.pool.close().await;
    std::fs::remove_file(path).unwrap();
}
