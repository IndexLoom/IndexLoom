use super::*;
async fn seed(store: &Store) {
    sqlx::query("INSERT INTO indexer_definitions(id,source,schema_version,content_yaml,content_sha256,imported_at) VALUES('def','test','1','','','')").execute(&store.pool).await.unwrap();
    sqlx::query("INSERT INTO indexers(id,definition_id,display_name,created_at,updated_at) VALUES('one','def','One','','')").execute(&store.pool).await.unwrap();
}
#[tokio::test]
async fn durable_counters_and_response_average() {
    let path = std::env::temp_dir().join(format!(
        "indexloom-monitoring-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let store = Store::connect_path(&path).await.unwrap();
    seed(&store).await;
    store
        .observe(Some("one"), Operation::Search, None, 100, 5)
        .await
        .unwrap();
    store
        .observe(
            Some("one"),
            Operation::Rss,
            Some(Failure::Authentication),
            300,
            0,
        )
        .await
        .unwrap();
    store
        .observe(Some("one"), Operation::Download, None, 200, 1)
        .await
        .unwrap();
    store
        .observe(None, Operation::Auth, Some(Failure::Authentication), 10, 0)
        .await
        .unwrap();
    store.pool.close().await;
    let reopened = Store::connect_path(&path).await.unwrap();
    let s = reopened.indexer_statistics().await.unwrap().remove(0);
    assert_eq!(
        (
            s.searches,
            s.downloads,
            s.rss,
            s.failed_queries,
            s.auth_errors
        ),
        (1, 1, 1, 1, 1)
    );
    assert_eq!(
        (s.response_count, s.response_sum_ms, s.results),
        (3, 600, 6)
    );
    assert!((s.average_response_ms - 200.0).abs() < f64::EPSILON);
    assert_eq!(reopened.global_authentication_errors().await.unwrap(), 1);
    for _ in 0..3 {
        reopened
            .observe(Some("one"), Operation::Test, Some(Failure::Network), 1, 0)
            .await
            .unwrap();
    }
    reopened.pool.close().await;
    let third = Store::connect_path(&path).await.unwrap();
    assert!(!third.claim_indexer_dispatch("one").await.unwrap());
    assert_eq!(
        third.indexer_statistics().await.unwrap()[0].consecutive_failures,
        3
    );
    third.pool.close().await;
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn cutoff_exclusive_probe_recovery_and_reset() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    seed(&s).await;
    let now = timestamp();
    for n in 0..3 {
        assert!(s.claim_dispatch_at("one", now).await.unwrap());
        s.observe_at(
            Some("one"),
            Operation::Search,
            Some(Failure::Network),
            10,
            0,
            now,
        )
        .await
        .unwrap();
        assert_eq!(
            s.indexer_statistics().await.unwrap()[0].consecutive_failures,
            n + 1
        );
    }
    s.observe_at(Some("one"), Operation::AuthPending, None, 10, 0, now)
        .await
        .unwrap();
    assert_eq!(
        s.indexer_statistics().await.unwrap()[0].consecutive_failures,
        3
    );
    assert!(!s.claim_dispatch_at("one", now).await.unwrap());
    assert!(s.claim_dispatch_at("one", now + 30).await.unwrap());
    assert!(!s.claim_dispatch_at("one", now + 30).await.unwrap());
    assert!(s.indexer_recovery_probe("one").await.unwrap());
    s.observe_at(
        Some("one"),
        Operation::Search,
        Some(Failure::RateLimited),
        20,
        0,
        now + 30,
    )
    .await
    .unwrap();
    assert_eq!(
        s.indexer_statistics().await.unwrap()[0].open_until,
        now + 270
    );
    assert!(s.claim_dispatch_at("one", now + 270).await.unwrap());
    s.observe_at(Some("one"), Operation::Search, None, 25, 2, now + 270)
        .await
        .unwrap();
    let r = s.indexer_statistics().await.unwrap().remove(0);
    assert_eq!(
        (r.consecutive_failures, r.open_until, r.probe_until),
        (0, 0, 0)
    );
    assert!(r.enabled);
    for _ in 0..40 {
        s.observe_at(
            Some("one"),
            Operation::Test,
            Some(Failure::Network),
            1,
            0,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        s.indexer_statistics().await.unwrap()[0].open_until,
        now + 3600
    );
    s.reset_indexer_circuit("one").await.unwrap();
    assert!(s.claim_dispatch_at("one", now).await.unwrap());
    assert!(s.reset_indexer_circuit("missing").await.is_err());
}
#[tokio::test]
async fn bounded_pages_filters_and_retention() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    seed(&s).await;
    let now = timestamp();
    // Seed old rows efficiently, then exercise the real transactional pruning path.
    sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10005) INSERT INTO monitoring_events(timestamp,category,status,message,count) SELECT ?, 'auth','error','authentication',0 FROM n").bind(now).execute(&s.pool).await.unwrap();
    s.observe(None, Operation::Auth, Some(Failure::Authentication), 0, 0)
        .await
        .unwrap();
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM monitoring_events")
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(total, EVENT_LIMIT);
    let page = s
        .monitoring_events(&EventFilter {
            limit: Some(99999),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.events.len(), 200);
    let next = s
        .monitoring_events(&EventFilter {
            before: page.next_before,
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(next.events[0].id < page.events.last().unwrap().id);
    assert!(
        s.monitoring_events(&EventFilter {
            category: Some("' OR 1=1 --".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .events
        .is_empty()
    );
    sqlx::query("UPDATE monitoring_events SET timestamp=?")
        .bind(now - EVENT_RETENTION_SECONDS - 1)
        .execute(&s.pool)
        .await
        .unwrap();
    s.observe(Some("one"), Operation::Search, None, 10, 3)
        .await
        .unwrap();
    let page = s.monitoring_events(&EventFilter::default()).await.unwrap();
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].message, "operation completed");
    assert!(s.monitoring_components().await.is_ok());
}

#[tokio::test]
async fn abandoned_probe_and_local_rejections_do_not_disable_preferences() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    seed(&s).await;
    let now = timestamp();
    assert_eq!(s.indexer_statistics().await.unwrap()[0].response_count, 0);
    assert!(!s.indexer_recovery_probe("one").await.unwrap());
    for _ in 0..4 {
        s.observe_at(
            Some("one"),
            Operation::Search,
            Some(Failure::Quota),
            0,
            0,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        s.indexer_statistics().await.unwrap()[0].consecutive_failures,
        0
    );
    for _ in 0..3 {
        s.observe_at(
            Some("one"),
            Operation::Search,
            Some(Failure::Network),
            10,
            0,
            now,
        )
        .await
        .unwrap();
    }
    assert!(s.claim_dispatch_at("one", now + 30).await.unwrap());
    assert!(!s.claim_dispatch_at("one", now + 329).await.unwrap());
    assert!(s.claim_dispatch_at("one", now + 330).await.unwrap());
    let (a, b) = tokio::join!(
        s.claim_dispatch_at("one", now + 630),
        s.claim_dispatch_at("one", now + 630)
    );
    assert_ne!(a.unwrap(), b.unwrap());
    s.reset_indexer_circuit("one").await.unwrap();
    let row = s.indexer_statistics().await.unwrap().remove(0);
    assert!(row.enabled);
    assert_eq!(row.consecutive_failures, 0);
    let events = s
        .monitoring_events(&EventFilter {
            indexer_id: Some("one".into()),
            category: Some("reset".into()),
            status: Some("success".into()),
            limit: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(events.events.len(), 1);
}

#[tokio::test]
async fn component_health_uses_saved_status_and_hides_connection_details() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO integrations(id,provider,name,origin,enabled) VALUES('app','sonarr','App','https://private-endpoint.invalid',1)").execute(&s.pool).await.unwrap();
    let unknown = s.monitoring_components().await.unwrap();
    assert_eq!(unknown["applications"][0]["status"], "unknown");
    s.integration_event("app", "test", false).await.unwrap();
    let failed = s.monitoring_components().await.unwrap();
    assert_eq!(failed["applications"][0]["status"], "degraded");
    assert!(!failed.to_string().contains("private-endpoint"));
    sqlx::query(
        "UPDATE integration_events SET outcome='success',created_at=datetime('now','-2 days')",
    )
    .execute(&s.pool)
    .await
    .unwrap();
    let stale = s.monitoring_components().await.unwrap();
    assert_eq!(stale["applications"][0]["status"], "stale");
    assert!(stale["applications"][0]["ageSeconds"].as_i64().unwrap() > 86400);
    sqlx::query("UPDATE integrations SET enabled=0")
        .execute(&s.pool)
        .await
        .unwrap();
    assert_eq!(
        s.monitoring_components().await.unwrap()["applications"][0]["status"],
        "disabled"
    );
}
