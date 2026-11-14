use super::{model::*, providers, *};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use indexloom_storage::{SecretVault, Store, notifications::Channel};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
use tower::ServiceExt;
const TOKEN: &str = "notification-tests-admin-token-32-bytes";
async fn state() -> AppState {
    AppState::new(
        Store::connect("sqlite::memory:").await.unwrap(),
        SecretVault::generate().unwrap(),
        crate::AdminToken::new(TOKEN).unwrap(),
    )
}
fn input(provider: &str) -> ChannelInput {
    ChannelInput {
        name: "Alerts".into(),
        provider: provider.into(),
        enabled: false,
        config: Config::default(),
        events: vec!["auth.login".into()],
        secrets: BTreeMap::new(),
        revision: 0,
    }
}
async fn channel(s: &AppState, provider: &str, url: &str) -> Channel {
    let c = Channel {
        id: uuid::Uuid::now_v7().to_string(),
        name: "Alerts".into(),
        provider: provider.into(),
        enabled: true,
        config: serde_json::to_string(&Config {
            allow_insecure_local: true,
            ..Config::default()
        })
        .unwrap(),
        events: r#"["auth.login"]"#.into(),
        secrets: BTreeMap::from([("url".into(), url.into())]),
        revision: 0,
    };
    assert!(
        s.store
            .save_notification_channel(&c, &s.vault)
            .await
            .unwrap()
    );
    s.store
        .notification_channel(&c.id, &s.vault)
        .await
        .unwrap()
        .unwrap()
}
async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Authorization", format!("Bearer {TOKEN}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[test]
fn url_policy_and_structured_validation() {
    for url in [
        "http://example.com",
        "https://user:password@example.com",
        "file:///etc/passwd",
        "https://example.com/#token",
        "http://169.254.169.254",
        "http://[::ffff:169.254.169.254]",
    ] {
        assert!(validate_url(url, true, true).is_err(), "{url}");
    }
    for url in [
        "http://127.0.0.1:8081",
        "http://localhost:8000",
        "http://192.168.1.12",
        "http://[::1]:8000",
        "http://[fd00::1]",
    ] {
        assert!(validate_url(url, true, true).is_ok());
        assert!(validate_url(url, false, true).is_err());
    }
    assert!(validate_url("https://example.com/?secret=x", false, true).is_err());
    assert!(validate_url("https://example.com/hook?key=x", false, false).is_ok());
    let mut i = input("pushover");
    i.config.priority = Some(2);
    assert!(validate(&i, &BTreeMap::new(), false).is_err()); // emergency needs retry/expire, deliberately unsupported
    i = input("telegram");
    i.events.push("password.reset".into());
    assert!(validate(&i, &BTreeMap::new(), false).is_err());
    i = input("gotify");
    i.config.chat_id = Some("123".into());
    assert!(validate(&i, &BTreeMap::new(), false).is_err());
    i = input("webhook");
    i.enabled = true;
    assert!(validate(&i, &BTreeMap::new(), false).is_err());
    assert!(serde_json::from_value::<Config>(json!({"token":"not-public"})).is_err());
    assert!(serde_json::from_value::<Config>(json!({"priority":1.5})).is_err());
}
#[test]
fn template_is_literal_single_pass_and_utf8_safe() {
    assert_eq!(
        render(
            "{{title}} {{unknown}} {{app}}",
            &[("title", "{{app}} <script>"), ("app", "IndexLoom")]
        ),
        "{{app}} <script> {{unknown}} IndexLoom"
    );
    assert_eq!(render("unclosed {{title", &[]), "unclosed {{title");
    assert_eq!(clip("Привет 🦀", 3), "При");
    assert_eq!(clip("🦀🦀", 3), "🦀");
    assert_eq!(clip_bytes("я🦀test", 5), "я");
    assert!(validate_template("\nBad", "body").is_err());
    assert!(validate_template("title", &"x".repeat(4001)).is_err());
}
#[test]
fn all_provider_requests_have_correct_credentials_and_plain_text_payloads() {
    let client = providers::client().unwrap();
    let m = Message {
        title: "Привет".into(),
        body: "<b>Тест</b>".into(),
        event: "test".into(),
        time: "now".into(),
    };
    let secrets = BTreeMap::from([
        ("token".into(), "123:bot_secret".into()),
        ("url".into(), "https://hooks.example/secret".into()),
        ("bearer".into(), "bearer_secret".into()),
        ("key".into(), "key_secret".into()),
        ("user".into(), "user_secret".into()),
    ]);
    let c = Config {
        api_base: Some("https://notify.example/prefix".into()),
        chat_id: Some("-100123".into()),
        thread_id: Some(42),
        topic: Some("topic".into()),
        ..Config::default()
    };
    for provider in [
        "telegram", "discord", "slack", "webhook", "ntfy", "gotify", "apprise", "pushover",
    ] {
        let r = providers::request(&client, provider, &c, &secrets, &m)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(r.method(), reqwest::Method::POST);
        assert!(!r.url().as_str().contains("bearer_secret"));
        let body = r.body().unwrap().as_bytes().unwrap();
        match provider {
            "telegram" => {
                assert_eq!(r.url().path(), "/prefix/bot123:bot_secret/sendMessage");
                let b: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(b["chat_id"], "-100123");
                assert_eq!(b["message_thread_id"], 42);
                assert!(b.get("parse_mode").is_none());
                assert!(b["text"].as_str().unwrap().contains("Тест"));
            }
            "discord" => {
                let b: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(b["allowed_mentions"]["parse"], json!([]));
            }
            "slack" => {
                let b: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(b["mrkdwn"], false);
            }
            "webhook" => {
                assert_eq!(r.headers()["authorization"], "Bearer bearer_secret");
                let b: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(b["event"], "test");
                assert_eq!(b["body"], m.body);
            }
            "ntfy" => {
                assert_eq!(r.url().path(), "/prefix/topic");
                assert_eq!(r.headers()["authorization"], "Bearer 123:bot_secret");
                assert_eq!(r.headers()["priority"], "3");
                assert!(
                    r.headers()["title"]
                        .to_str()
                        .unwrap()
                        .starts_with("=?UTF-8?B?")
                );
            }
            "gotify" => {
                assert_eq!(r.url().path(), "/prefix/message");
                assert_eq!(r.headers()["X-Gotify-Key"], "123:bot_secret");
            }
            "apprise" => {
                assert_eq!(r.url().path(), "/secret/notify/key_secret/");
                let b: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(b["format"], "text");
            }
            "pushover" => {
                assert_eq!(r.url().as_str(), "https://api.pushover.net/1/messages.json");
                let body = std::str::from_utf8(body).unwrap();
                assert!(body.contains("token=123%3Abot_secret"));
                assert!(body.contains("user=user_secret"));
            }
            _ => unreachable!(),
        }
    }
}
async fn mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (
        format!("http://{addr}"),
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}
#[tokio::test]
async fn api_secrets_preserve_replace_clear_and_switch_with_revision() {
    let s = state().await;
    let app = crate::router(s.clone(), PathBuf::new());
    let mut body = json!({"name":"Alerts","provider":"webhook","enabled":true,"config":{},"events":["auth.login"],"secrets":{"url":"https://example.com/top-secret?token=hidden","bearer":"secret-bearer"}});
    let (status, c) = call(&app, "POST", "/api/v1/notifications/channels", body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let id = c["id"].as_str().unwrap();
    let path = format!("/api/v1/notifications/channels/{id}");
    let (_, list) = call(&app, "GET", "/api/v1/notifications/channels", Value::Null).await;
    let public = serde_json::to_string(&list).unwrap();
    for secret in ["top-secret", "hidden", "secret-bearer"] {
        assert!(!public.contains(secret));
    }
    assert_eq!(c["storedSecrets"], json!(["bearer", "url"]));
    body["revision"] = c["revision"].clone();
    body["secrets"] = json!({});
    body["name"] = json!("Renamed");
    let (status, c) = call(&app, "PUT", &path, body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        s.store
            .notification_channel(id, &s.vault)
            .await
            .unwrap()
            .unwrap()
            .secrets["bearer"],
        "secret-bearer"
    );
    let (stale, _) = call(&app, "PUT", &path, body.clone()).await;
    assert_eq!(stale, StatusCode::CONFLICT);
    body["revision"] = c["revision"].clone();
    body["secrets"] = json!({"bearer":"replacement"});
    let (_, c) = call(&app, "PUT", &path, body.clone()).await;
    assert_eq!(
        s.store
            .notification_channel(id, &s.vault)
            .await
            .unwrap()
            .unwrap()
            .secrets["bearer"],
        "replacement"
    );
    body["revision"] = c["revision"].clone();
    body["secrets"] = json!({"bearer":null});
    let (_, c) = call(&app, "PUT", &path, body.clone()).await;
    assert!(
        !s.store
            .notification_channel(id, &s.vault)
            .await
            .unwrap()
            .unwrap()
            .secrets
            .contains_key("bearer")
    );
    body["revision"] = c["revision"].clone();
    body["provider"] = json!("telegram");
    body["enabled"] = json!(false);
    body["secrets"] = json!({});
    let (status, c) = call(&app, "PUT", &path, body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["storedSecrets"], json!([]));
    let (status, _) = call(&app, "GET", "/api/v1/notifications/templates", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/notifications/templates/auth.login",
        json!({"title":"{{app}}","body":"{{event}}"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, p) = call(
        &app,
        "POST",
        "/api/v1/notifications/preview",
        json!({"event":"auth.login","title":"{{app}}","body":"{{event}}"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["title"], "IndexLoom");
    assert_eq!(p["body"], "auth.login");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/notifications/channels")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn real_test_endpoint_sends_once_logs_result_and_redacts_errors() {
    let (url, server) = mock(
        Router::new()
            .route("/ok", post(|| async { StatusCode::NO_CONTENT }))
            .route(
                "/fail",
                post(|| async { (StatusCode::BAD_REQUEST, "secret-url-and-token") }),
            ),
    )
    .await;
    let s = state().await;
    let c = channel(&s, "webhook", &format!("{url}/ok")).await;
    let bad = channel(&s, "webhook", &format!("{url}/fail")).await;
    let app = crate::router(s.clone(), PathBuf::new());
    let (status, r) = call(
        &app,
        "POST",
        &format!("/api/v1/notifications/channels/{}/test", c.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["success"], true);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/notifications/channels/{}/test", c.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (_, r) = call(
        &app,
        "POST",
        &format!("/api/v1/notifications/channels/{}/test", bad.id),
        Value::Null,
    )
    .await;
    assert_eq!(r["success"], false);
    assert!(!r.to_string().contains("secret-url"));
    let (_, logs) = call(
        &app,
        "GET",
        "/api/v1/notifications/deliveries?limit=1",
        Value::Null,
    )
    .await;
    assert_eq!(logs.as_array().unwrap().len(), 1);
    assert!(!logs.to_string().contains(&url));
    assert!(!logs.to_string().contains("secret-url"));
    server.abort();
}
#[tokio::test]
async fn worker_filters_deduplicates_applies_templates_and_retries_transient_errors() {
    let (url, server) = mock(
        Router::new()
            .fallback(|| async { (StatusCode::SERVICE_UNAVAILABLE, "sensitive diagnostic") }),
    )
    .await;
    let s = state().await;
    let c = channel(&s, "webhook", &url).await;
    s.store
        .save_notification_template(&Template {
            scope: String::new(),
            event: "auth.login".into(),
            title: "global".into(),
            body: "{{message}}".into(),
        })
        .await
        .unwrap();
    s.store
        .save_notification_template(&Template {
            scope: c.id.clone(),
            event: "auth.login".into(),
            title: "override".into(),
            body: "{{event}}".into(),
        })
        .await
        .unwrap();
    emit(&s, Event::new("system.startup", "unsubscribed")).await;
    emit(&s, Event::new("auth.login", "one-event")).await;
    emit(&s, Event::new("auth.login", "one-event")).await;
    assert_eq!(
        s.store
            .notification_deliveries(None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let pending = s
        .store
        .claim_notification("worker", None)
        .await
        .unwrap()
        .unwrap();
    let msg: Message = serde_json::from_str(&pending.payload).unwrap();
    assert_eq!(msg.title, "override");
    assert_eq!(msg.body, "auth.login");
    assert!(
        s.store
            .claim_notification("other-worker", None)
            .await
            .unwrap()
            .is_none()
    );
    let r = deliver(&s, &providers::client().unwrap(), pending, "worker", false)
        .await
        .unwrap();
    assert_eq!(r.status, "retry");
    let logs = s.store.notification_deliveries(None, 100).await.unwrap();
    assert_eq!(logs[0].attempts, 1);
    assert!(!serde_json::to_string(&logs).unwrap().contains("sensitive"));
    assert!(
        s.store
            .claim_notification("worker", None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(backoff(1), 30);
    assert_eq!(backoff(4), 240);
    assert_eq!(backoff(5), 480);
    server.abort();
}
#[tokio::test]
async fn api_errors_and_redirects_never_count_as_success() {
    let (url, server) = mock(
        Router::new()
            .route(
                "/telegram/bot123:abc/sendMessage",
                post(|| async { Json(json!({"ok":false,"description":"secret"})) }),
            )
            .route(
                "/gotify/message",
                post(|| async { Json(json!({"error":"secret","errorCode":400})) }),
            )
            .route(
                "/redirect",
                post(|| async { axum::response::Redirect::temporary("/success") }),
            )
            .route("/success", post(|| async { StatusCode::NO_CONTENT })),
    )
    .await;
    let s = state().await;
    let mut c = channel(&s, "webhook", &format!("{url}/redirect")).await;
    let m = Message {
        title: "Test".into(),
        body: "Hello".into(),
        event: "test".into(),
        time: "now".into(),
    };
    let client = providers::client().unwrap();
    let failure = providers::send(&client, &c, &m).await.unwrap_err();
    assert!(!failure.retryable);
    assert_eq!(failure.summary, "Provider redirect refused");
    for provider in ["telegram", "gotify"] {
        c.provider = provider.into();
        c.secrets = BTreeMap::from([("token".into(), "123:abc".into())]);
        c.config = serde_json::to_string(&Config {
            api_base: Some(format!("{url}/{provider}")),
            chat_id: Some("1".into()),
            ..Config::default()
        })
        .unwrap();
        assert_eq!(
            providers::send(&client, &c, &m).await.unwrap_err().summary,
            "Provider reported an API error"
        );
    }
    server.abort();
}

#[tokio::test]
async fn invalid_json_never_echoes_credential_values_and_metadata_is_not_cached() {
    let s = state().await;
    let app = crate::router(s, PathBuf::new());
    let (status,body)=call(&app,"POST","/api/v1/notifications/channels",json!({"name":"test","provider":"telegram","enabled":false,"events":[],"config":{},"secrets":{"token":123_456_789_123_456_i64}})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains("123456789123456"));
    assert_eq!(body["code"], "INVALID_CONFIGURATION");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/notifications/events")
                .header("Authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store, private");
}
#[tokio::test]
async fn claimed_messages_cannot_be_sent_using_an_edited_destination() {
    let s = state().await;
    let mut c = channel(&s, "webhook", "https://example.invalid/never-called").await;
    emit(&s, Event::new("auth.login", "revision-race")).await;
    let pending = s
        .store
        .claim_notification("lease", None)
        .await
        .unwrap()
        .unwrap();
    c.name = "Edited".into();
    assert!(
        s.store
            .save_notification_channel(&c, &s.vault)
            .await
            .unwrap()
    );
    let result = deliver(&s, &providers::client().unwrap(), pending, "lease", false)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.summary, "Channel configuration changed");
}
#[tokio::test]
async fn daemon_worker_drains_the_durable_outbox() {
    let (url, server) = mock(Router::new().fallback(|| async { StatusCode::NO_CONTENT })).await;
    let s = state().await;
    let _c = channel(&s, "webhook", &url).await;
    emit(&s, Event::new("auth.login", "worker-event")).await;
    let worker = tokio::spawn(run(s.clone()));
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if s.store
                .notification_deliveries(None, 10)
                .await
                .unwrap()
                .iter()
                .any(|d| d.status == "sent")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    worker.abort();
    server.abort();
    result.unwrap();
}
