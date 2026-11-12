//! Tests for the parent module; extracted for easier maintenance.
use super::Store;

    #[tokio::test]
    async fn bootstrap_is_one_time_and_sessions_are_revocable() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("create test database");
        assert_eq!(store.auth_user_count().await.unwrap(), 0);
        assert!(
            store
                .bootstrap_admin("alice", "alice", "alice@example.org", "hash", true)
                .await
                .unwrap()
        );
        assert!(
            !store
                .bootstrap_admin("other", "other", "other@example.org", "hash", true)
                .await
                .unwrap()
        );
        let user = store
            .auth_user_by_login("ALICE@example.org")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.username, "alice");
        store.create_session("digest", &user.id, 60).await.unwrap();
        assert!(
            store
                .auth_user_for_session("digest")
                .await
                .unwrap()
                .is_some()
        );
        store.revoke_session("digest").await.unwrap();
        assert!(
            store
                .auth_user_for_session("digest")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn one_time_token_is_consumed_only_once() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        store
            .bootstrap_admin("id", "alice", "alice@example.org", "hash", false)
            .await
            .unwrap();
        store
            .create_one_time_token("digest", "id", "verify-email", 60)
            .await
            .unwrap();
        assert_eq!(
            store
                .consume_one_time_token("digest", "verify-email")
                .await
                .unwrap()
                .as_deref(),
            Some("id")
        );
        assert!(
            store
                .consume_one_time_token("digest", "verify-email")
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn owner_admin_viewer_permissions_are_persistent_and_last_owner_is_safe() {
        let store = Store::connect("sqlite::memory:")
            .await
            .expect("migrate account roles");
        assert!(
            store
                .bootstrap_admin("owner", "first", "first@example.org", "argon2", true,)
                .await
                .unwrap()
        );
        let initial = store.auth_user_by_id("owner").await.unwrap().unwrap();
        assert_eq!(initial.role, "owner");
        assert!(!store.set_user_role("owner", "viewer").await.unwrap());
        assert!(!store.set_user_disabled("owner", true).await.unwrap());

        store
            .add_local_user(
                "reader",
                "reader",
                "reader@example.org",
                "argon2",
                true,
                "viewer",
            )
            .await
            .unwrap();
        assert_eq!(
            store.auth_user_by_id("reader").await.unwrap().unwrap().role,
            "viewer"
        );
        assert!(store.set_user_role("reader", "admin").await.unwrap());
        assert_eq!(
            store.auth_user_by_id("reader").await.unwrap().unwrap().role,
            "admin"
        );
        assert!(store.set_user_role("reader", "owner").await.unwrap());
        assert!(store.set_user_role("owner", "viewer").await.unwrap());
        assert!(!store.set_user_disabled("reader", true).await.unwrap());
        assert!(store.set_user_role("owner", "owner").await.unwrap());
        assert!(store.set_user_disabled("reader", true).await.unwrap());
        assert!(
            store
                .auth_user_by_id("reader")
                .await
                .unwrap()
                .unwrap()
                .disabled
        );

        store
            .auth_audit(Some("owner"), Some("reader"), "role-changed")
            .await
            .unwrap();
        let events = store.auth_audit_events().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "role-changed");
        assert_eq!(events[0].actor_id.as_deref(), Some("owner"));
        assert!(!store.set_user_role("owner", "invalid").await.unwrap());
    }

    #[tokio::test]
    async fn configurable_password_rules_persist_and_hash_history_is_bounded() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        assert!(store.load_password_policy().await.unwrap().is_none());
        store
            .save_password_policy(
                r#"{"minimumLength":16,"uppercase":{"enabled":true,"minimum":2}}"#,
            )
            .await
            .unwrap();
        assert!(
            store
                .load_password_policy()
                .await
                .unwrap()
                .unwrap()
                .contains("minimumLength")
        );
        store
            .bootstrap_admin(
                "policy-owner",
                "policy-owner",
                "policy@example.org",
                "hash-0",
                true,
            )
            .await
            .unwrap();
        store
            .create_one_time_token("reset-digest", "policy-owner", "reset-password", 600)
            .await
            .unwrap();
        assert_eq!(
            store
                .password_reset_user("reset-digest")
                .await
                .unwrap()
                .as_deref(),
            Some("policy-owner")
        );
        assert_eq!(store.password_reset_user("invalid").await.unwrap(), None);
        for i in 1..=12 {
            store
                .change_password_with_history("policy-owner", &format!("hash-{i}"), true)
                .await
                .unwrap();
        }
        let recent = store
            .password_hash_history("policy-owner", 10)
            .await
            .unwrap();
        assert_eq!(recent.len(), 11);
        assert_eq!(recent[0], "hash-12");
        assert_eq!(recent[1], "hash-11");
        assert_eq!(recent[10], "hash-2");
        assert!(!recent.contains(&"hash-0".to_owned()));
        // Turning off password history must also delete retained hashes.
        store
            .save_password_policy(r#"{"historyEnabled":false}"#)
            .await
            .unwrap();
        assert_eq!(
            store
                .password_hash_history("policy-owner", 10)
                .await
                .unwrap(),
            vec!["hash-12".to_owned()]
        );
        store
            .change_password("policy-owner", "hash-13")
            .await
            .unwrap();
        assert_eq!(
            store
                .password_hash_history("policy-owner", 10)
                .await
                .unwrap(),
            vec!["hash-13".to_owned()]
        );
        assert_eq!(
            store
                .password_hash_history("policy-owner", 0)
                .await
                .unwrap(),
            vec!["hash-13".to_owned()]
        );
        assert_eq!(
            store
                .consume_one_time_token("reset-digest", "reset-password")
                .await
                .unwrap()
                .as_deref(),
            Some("policy-owner")
        );
        assert!(
            store
                .password_reset_user("reset-digest")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn explicit_owner_role_is_not_based_on_oidc_login_order() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        store
            .add_oidc_user(
                "oidc-primary",
                "oidc-one",
                "one@example.org",
                "https://issuer.example.org",
                "subject-1",
                "viewer",
            )
            .await
            .unwrap();
        store
            .add_oidc_user(
                "oidc-secondary",
                "oidc-two",
                "two@example.org",
                "https://issuer.example.org",
                "subject-2",
                "owner",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .auth_user_by_id("oidc-primary")
                .await
                .unwrap()
                .unwrap()
                .role,
            "viewer"
        );
        assert_eq!(
            store
                .auth_user_by_id("oidc-secondary")
                .await
                .unwrap()
                .unwrap()
                .role,
            "owner"
        );
    }
