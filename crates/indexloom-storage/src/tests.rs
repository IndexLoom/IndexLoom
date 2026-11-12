//! Tests for the parent module; extracted for easier maintenance.
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
