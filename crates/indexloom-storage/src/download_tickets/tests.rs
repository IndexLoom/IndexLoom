use super::*;

async fn fixture() -> (Store, SecretVault) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let vault = SecretVault::generate().unwrap();
    store
        .upsert_definition("fixture", "test", None, "v1", "fixture", &"0".repeat(64))
        .await
        .unwrap();
    store
        .create_indexer("source-a", "fixture", "A", true)
        .await
        .unwrap();
    store
        .create_indexer("source-b", "fixture", "B", true)
        .await
        .unwrap();
    (store, vault)
}
#[tokio::test]
async fn tickets_encrypt_private_links_and_resolve_stable_guid() {
    let (store, vault) = fixture().await;
    let release = r#"{"title":"sample","downloadUrl":"https://tracker.example/get?passkey=never-store-plaintext"}"#;
    let guid = "a".repeat(64);
    let id = store
        .create_download_ticket("source-a", release, &guid, &vault)
        .await
        .unwrap();
    assert_eq!(id.len(), 64);
    assert_eq!(
        store.download_ticket_for_guid(&guid).await.unwrap(),
        Some(id.clone())
    );
    assert_eq!(
        store.load_download_ticket(&id, &vault).await.unwrap(),
        Some(("source-a".into(), release.into()))
    );
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM download_tickets WHERE id=?")
            .bind(&id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(
        !ciphertext
            .windows(b"never-store-plaintext".len())
            .any(|bytes| bytes == b"never-store-plaintext")
    );
    assert!(
        store
            .load_download_ticket(&id, &SecretVault::generate().unwrap())
            .await
            .is_err()
    );
    assert!(
        store
            .load_download_ticket(&"f".repeat(64), &vault)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn tickets_expire_or_revoke_when_disabled_or_configuration_changes() {
    let (store, vault) = fixture().await;
    let guid = "b".repeat(64);
    let id = store
        .create_download_ticket("source-a", "{}", &guid, &vault)
        .await
        .unwrap();
    sqlx::query("UPDATE indexers SET session_revision=session_revision+1 WHERE id='source-a'")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_download_ticket(&id, &vault)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .download_ticket_for_guid(&guid)
            .await
            .unwrap()
            .is_none()
    );
    let id = store
        .create_download_ticket("source-a", "{}", &guid, &vault)
        .await
        .unwrap();
    sqlx::query("UPDATE indexers SET enabled=0 WHERE id='source-a'")
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_download_ticket(&id, &vault)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE indexers SET enabled=1 WHERE id='source-a'")
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE download_tickets SET expires_at=0 WHERE id=?")
        .bind(&id)
        .execute(&store.pool)
        .await
        .unwrap();
    assert!(
        store
            .load_download_ticket(&id, &vault)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn custom_newznab_source_secrets_and_ids_are_kept_separate() {
    let (store, vault) = fixture().await;
    let source = "usenet-sample";
    let key = "private-newznab-token-12345";
    store
        .upsert_newznab_indexer(
            source,
            "Sample",
            "{}",
            &"b".repeat(64),
            true,
            Some(key),
            &vault,
        )
        .await
        .unwrap();
    let record = store.get_indexer(source, &vault).await.unwrap();
    assert!(
        record
            .configured_secrets
            .iter()
            .any(|name| name == "apiKey")
    );
    assert!(!record.definition_yaml.contains(key));
    let resolved = store
        .resolve_indexer_configuration(source, &vault)
        .await
        .unwrap();
    assert_eq!(resolved.secrets.get("apiKey").unwrap(), key);
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM indexer_secrets WHERE indexer_id=?")
            .bind(source)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(
        !ciphertext
            .windows(key.len())
            .any(|window| window == key.as_bytes())
    );
    store
        .upsert_newznab_indexer(
            source,
            "Renamed",
            "{}",
            &"c".repeat(64),
            false,
            None,
            &vault,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .resolve_indexer_configuration(source, &vault)
            .await
            .unwrap()
            .secrets
            .get("apiKey")
            .unwrap(),
        key
    );
    assert!(store.delete_newznab_indexer("fixture").await.is_err());
    store.delete_newznab_indexer(source).await.unwrap();
    assert!(store.get_indexer(source, &vault).await.is_err());
}
