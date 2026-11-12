//! Tests for the parent module; extracted for easier maintenance.
use std::{collections::BTreeMap, time::Instant};

use indexloom_storage::{
    IndexerConfigurationRecord, ProxyKind, ResolvedIndexerConfiguration, ResolvedOutboundProxy,
};
use serde_json::json;

use super::{
    MAX_COMPLETED_TEST_JOBS, MAX_RUNNING_TEST_JOBS, TEST_JOB_TTL, TestJob, TestJobRegistry,
    TestJobStatus, execution_fingerprint, prune_test_jobs, validate_search_languages,
};

#[test]
fn validates_international_indexer_language_tags() {
    assert!(
        validate_search_languages(&["ru".to_owned(), "ja-JP".to_owned(), "zh-Hans".to_owned()])
            .is_ok()
    );
    assert!(validate_search_languages(&[]).is_ok());
    for invalid in ["", "a", "en_US", "../etc/passwd", "-ru", "fr-", "ру"] {
        assert!(
            validate_search_languages(&[invalid.to_owned()]).is_err(),
            "{invalid} should be rejected"
        );
    }
    assert!(validate_search_languages(&vec!["de".to_owned(); 17]).is_err());
}

#[test]
fn prunes_expired_and_oldest_completed_jobs_but_keeps_running_jobs() {
    let mut jobs = BTreeMap::new();
    jobs.insert(
        "expired".to_owned(),
        completed_job("expired", TEST_JOB_TTL + std::time::Duration::from_secs(1)),
    );
    jobs.insert("running".to_owned(), running_job("running"));
    for index in 0..(MAX_COMPLETED_TEST_JOBS + 5) {
        let id = format!("completed-{index:04}");
        let age = std::time::Duration::from_secs(
            u64::try_from(MAX_COMPLETED_TEST_JOBS + 5 - index).unwrap(),
        );
        jobs.insert(id.clone(), completed_job(&id, age));
    }

    prune_test_jobs(&mut jobs);

    assert!(!jobs.contains_key("expired"));
    assert!(jobs.contains_key("running"));
    assert_eq!(
        jobs.values()
            .filter(|job| job.status == TestJobStatus::Completed)
            .count(),
        MAX_COMPLETED_TEST_JOBS
    );
    assert!(!jobs.contains_key("completed-0000"));
    assert!(jobs.contains_key(&format!("completed-{:04}", MAX_COMPLETED_TEST_JOBS + 4)));
}

#[tokio::test]
async fn rejects_more_than_the_running_test_job_budget() {
    let registry = TestJobRegistry::default();
    for index in 0..MAX_RUNNING_TEST_JOBS {
        registry
            .insert(running_job(&format!("running-{index}")))
            .await
            .expect("job within the active budget should be accepted");
    }
    let error = registry
        .insert(running_job("one-too-many"))
        .await
        .expect_err("active job budget must be enforced");
    assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn runtime_fingerprint_changes_with_credentials_and_effective_proxy() {
    let record = IndexerConfigurationRecord {
        id: "example-main".to_owned(),
        definition_id: "example".to_owned(),
        display_name: "Example".to_owned(),
        enabled: true,
        catalog_available: true,
        user_agent: None,
        flaresolverr_instance_id: None,
        proxy_id: None,
        group_ids: Vec::new(),
        tags: Vec::new(),
        last_test: None,
        values: BTreeMap::new(),
        configured_secrets: Vec::new(),
        definition_yaml: "definition-v1".to_owned(),
    };
    let mut resolved = ResolvedIndexerConfiguration {
        user_agent: None,
        flaresolverr_instance_id: None,
        proxy_id: None,
        values: BTreeMap::from([("ratio".to_owned(), json!(1))]),
        secrets: BTreeMap::from([("cookie".to_owned(), "first".to_owned())]),
    };
    let direct = execution_fingerprint(&record, &resolved, None, None).unwrap();
    resolved
        .secrets
        .insert("cookie".to_owned(), "second".to_owned());
    let changed_secret = execution_fingerprint(&record, &resolved, None, None).unwrap();
    assert_ne!(direct, changed_secret);

    let proxy = ResolvedOutboundProxy {
        id: "proxy-main".to_owned(),
        kind: ProxyKind::Socks5,
        host: "proxy.example".to_owned(),
        port: 1080,
        enabled: true,
        username: Some("alice".to_owned()),
        password: Some("secret".to_owned()),
    };
    let proxied = execution_fingerprint(&record, &resolved, Some(&proxy), None).unwrap();
    assert_ne!(changed_secret, proxied);
}

fn running_job(id: &str) -> TestJob {
    TestJob {
        id: id.to_owned(),
        status: TestJobStatus::Running,
        total: 1,
        completed: 0,
        results: Vec::new(),
        created_at: "2026-08-26T00:00:00Z".to_owned(),
        finished_at: None,
        finished_monotonic: None,
    }
}

fn completed_job(id: &str, age: std::time::Duration) -> TestJob {
    TestJob {
        id: id.to_owned(),
        status: TestJobStatus::Completed,
        total: 1,
        completed: 1,
        results: Vec::new(),
        created_at: "2026-08-26T00:00:00Z".to_owned(),
        finished_at: Some("2026-08-26T00:00:01Z".to_owned()),
        finished_monotonic: Some(
            Instant::now()
                .checked_sub(age)
                .expect("test age is bounded"),
        ),
    }
}
