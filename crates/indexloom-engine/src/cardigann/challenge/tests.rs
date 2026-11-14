use super::super::{IndexerDefinition, JsonValue};
use super::*;
use crate::{IndexerExecutionConfig, ProxyRoute};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct Mock {
    image_reads: Arc<AtomicUsize>,
    posts: Arc<AtomicUsize>,
    sessions: Arc<AtomicUsize>,
    mode: &'static str,
}
struct Fixture {
    executor: CardigannExecutor,
    mock: Mock,
    task: tokio::task::JoinHandle<()>,
    proxy: ProxyRoute,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Single mock tracker reproduces all challenge error cases."
)]
async fn handler(State(mock): State<Mock>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let cookies = request
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let referer = request
        .headers()
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = to_bytes(request.into_body(), 65536).await.unwrap();
    let form = url::form_urlencoded::parse(&body)
        .into_owned()
        .collect::<BTreeMap<_, _>>();
    match path.as_str() {
        "/login" => {
            let session = mock.sessions.fetch_add(1, Ordering::SeqCst) + 1;
            let captcha = match mock.mode {
                "text" => "<p id=captcha>What is two plus three?</p><input name=answer>",
                "plain" => "",
                "browser" => "<div class=g-recaptcha></div>",
                "pin" => "<input name=pin>",
                "custom" => "<input name=security_word class=manual>",
                _ => "<img id=captcha src=/image><input name=answer>",
            };
            ([("set-cookie", format!("sid={session}; Path=/; HttpOnly"))], format!("<form id=login action=/submit><input type=hidden name=nonce value=hidden-secret><input name=username><input name=password type=password>{captcha}</form>")).into_response()
        }
        "/image" => {
            assert!(
                cookies.contains("sid="),
                "image must use login cookie session"
            );
            assert_eq!(referer, "http://tracker.invalid/login");
            mock.image_reads.fetch_add(1, Ordering::SeqCst);
            if mock.mode == "redirect" {
                return (
                    StatusCode::FOUND,
                    [("location", "http://127.0.0.1/private")],
                )
                    .into_response();
            }
            if mock.mode == "svg" {
                return ([("content-type", "image/svg+xml")], "<svg onload=evil() />")
                    .into_response();
            }
            if mock.mode == "html" {
                return ([("content-type", "image/png")], "<html>login</html>").into_response();
            }
            (
                [
                    ("content-type", "image/png"),
                    ("set-cookie", "image_nonce=ready; Path=/"),
                ],
                b"\x89PNG\r\n\x1a\nfixture-raster".to_vec(),
            )
                .into_response()
        }
        "/submit" => {
            mock.posts.fetch_add(1, Ordering::SeqCst);
            assert!(cookies.contains("sid="));
            assert_eq!(form.get("nonce").map(String::as_str), Some("hidden-secret"));
            assert_eq!(form.get("username").map(String::as_str), Some("test-user"));
            assert_eq!(
                form.get("password").map(String::as_str),
                Some("secret-password")
            );
            let correct = match mock.mode {
                "pin" => form.get("pin").is_some_and(|s| s == "1234"),
                "custom" => form.get("security_word").is_some_and(|s| s == "blue"),
                "plain" => true,
                _ => form.get("answer").is_some_and(|s| s == "five"),
            };
            if !correct {
                return if mock.mode == "repeat" {
                    "<form><img id=captcha src=/image><input name=answer></form>".into_response()
                } else {
                    "<div class=error>wrong response</div>".into_response()
                };
            }
            if !matches!(mock.mode, "plain" | "text" | "pin" | "custom") {
                assert!(cookies.contains("image_nonce=ready"));
            }
            if mock.mode == "postredirect" {
                return (
                    StatusCode::FOUND,
                    [("location", "http://127.0.0.1/private")],
                )
                    .into_response();
            }
            if matches!(mock.mode, "multi" | "plain") {
                return "<form action=/otp><input type=hidden name=stage value=second><input name=otp autocomplete=one-time-code></form>".into_response();
            }
            ([("set-cookie", "authenticated=1; Path=/")], "ok").into_response()
        }
        "/otp" => {
            assert!(cookies.contains("sid="));
            assert_eq!(form.get("stage").map(String::as_str), Some("second"));
            assert!(
                !form.contains_key("password"),
                "initial credentials must not leak to next step"
            );
            if form.get("otp").is_some_and(|s| s == "123456") {
                ([("set-cookie", "authenticated=1; Path=/")], "ok").into_response()
            } else {
                "<div class=error>wrong OTP</div>".into_response()
            }
        }
        "/test" => {
            if cookies.contains("authenticated=1") {
                "<div class=authenticated>ok</div>".into_response()
            } else {
                "unauthenticated".into_response()
            }
        }
        "/search" => "<html></html>".into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn fixture(mode: &'static str) -> Fixture {
    let mock = Mock {
        mode,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .fallback(any(handler))
        .with_state(mock.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let proxy = ProxyRoute {
        id: "offline-mock".into(),
        url: format!("http://127.0.0.1:{port}"),
        enabled: true,
        username: None,
        password: None,
    };
    let executor = executor(&proxy, mode, "tracker-one");
    Fixture {
        executor,
        mock,
        task,
        proxy,
    }
}
fn executor(proxy: &ProxyRoute, mode: &str, id: &str) -> CardigannExecutor {
    let mut definition: IndexerDefinition =
        serde_yaml::from_str(include_str!("../../../tests/fixtures/interactive.yml")).unwrap();
    let auth = definition
        .spec
        .implementation
        .authentication
        .as_mut()
        .unwrap()
        .as_mapping_mut()
        .unwrap();
    if mode == "text" {
        auth.get_mut(key("captcha"))
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .insert(key("type"), key("text"));
        auth.get_mut(key("captcha"))
            .unwrap()
            .as_mapping_mut()
            .unwrap()
            .insert(key("selector"), key("#captcha"));
    }
    if matches!(mode, "plain" | "pin" | "custom") {
        auth.remove(key("captcha"));
    }
    if mode == "custom" {
        auth.insert(
            key("interactive"),
            serde_yaml::from_str("[{selector: '.manual', label: 'Security word'}]").unwrap(),
        );
    }
    CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: id.into(),
        definition,
        values: BTreeMap::from([("username".into(), JsonValue::String("test-user".into()))]),
        secrets: BTreeMap::from([("password".into(), "secret-password".into())]),
        user_agent: None,
        proxy: Some(proxy.clone()),
        flaresolverr: None,
    })
    .unwrap()
}
fn pending(progress: LoginProgress) -> LoginChallengeDescriptor {
    match progress {
        LoginProgress::Pending { challenge } => challenge,
        LoginProgress::Complete => panic!("expected pending challenge"),
    }
}
fn answer(name: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(name.into(), value.into())])
}

#[tokio::test]
async fn fetch_submit_and_reuse_with_current_executor() {
    let f = fixture("image").await;
    assert!(!f.executor.test().await.success);
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    assert_eq!(p.id.len(), 64);
    assert!(p.has_image);
    assert_eq!(f.mock.image_reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        pending(f.executor.begin_interactive_login("alice").await.unwrap()).id,
        p.id
    );
    let (mime, bytes) = f
        .executor
        .login_challenge_image("alice", &p.id)
        .await
        .unwrap();
    assert_eq!(mime, "image/png");
    assert!(bytes.starts_with(b"\x89PNG"));
    let serialized = serde_json::to_string(&p).unwrap();
    for secret in [
        "hidden-secret",
        "secret-password",
        "tracker.invalid",
        "sid=",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert!(matches!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await
            .unwrap(),
        LoginProgress::Complete
    ));
    assert!(f.executor.test().await.success);
    assert!(f.executor.search("test", 10).await.is_ok());
    assert!(matches!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await,
        Err(EngineError::LoginChallengeUnavailable)
    ));
    assert!(
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .is_err()
    );
    assert_eq!(f.mock.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn owner_and_indexer_sessions_are_isolated() {
    let f = fixture("image").await;
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    assert!(
        f.executor
            .login_challenge_image("bob", &p.id)
            .await
            .is_err()
    );
    assert!(
        f.executor
            .submit_login_challenge("bob", &p.id, answer("answer", "five"))
            .await
            .is_err()
    );
    assert!(f.executor.begin_interactive_login("bob").await.is_err());
    let second = executor(&f.proxy, "image", "tracker-two");
    assert!(second.login_challenge_image("alice", &p.id).await.is_err());
    let q = pending(second.begin_interactive_login("alice").await.unwrap());
    assert_ne!(p.id, q.id);
    assert_eq!(f.mock.sessions.load(Ordering::SeqCst), 2);
    f.executor
        .submit_login_challenge("alice", &p.id, answer("answer", "five"))
        .await
        .unwrap();
    assert!(!second.test().await.success);
}

#[tokio::test]
async fn expired_wrong_and_malformed_answers_are_consumed() {
    for mode in ["image", "repeat"] {
        let f = fixture(mode).await;
        let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
        assert!(matches!(
            f.executor
                .submit_login_challenge("alice", &p.id, answer("answer", "wrong"))
                .await,
            Err(EngineError::AuthenticationRejected)
        ));
        assert!(
            f.executor
                .submit_login_challenge("alice", &p.id, answer("answer", "five"))
                .await
                .is_err()
        );
    }
    let f = fixture("image").await;
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    assert!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("password", "override"))
            .await
            .is_err()
    );
    assert!(
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .is_err()
    );
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    f.executor
        .auth_state
        .lock()
        .await
        .interactive
        .pending
        .as_mut()
        .unwrap()
        .started = Instant::now().checked_sub(TTL).unwrap();
    assert!(
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .is_err()
    );
    assert!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await
            .is_err()
    );
    assert_eq!(f.mock.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn image_and_form_hostile_redirects_and_active_content_are_rejected() {
    for mode in ["redirect", "svg", "html", "browser"] {
        let f = fixture(mode).await;
        assert!(
            f.executor.begin_interactive_login("alice").await.is_err(),
            "{mode}"
        );
        assert_eq!(f.mock.posts.load(Ordering::SeqCst), 0);
    }
    let f = fixture("postredirect").await;
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    assert!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await
            .is_err()
    );
    assert!(
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rate_limits_image_reads_and_login_attempts() {
    let f = fixture("image").await;
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    for _ in 0..30 {
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .unwrap();
    }
    assert!(matches!(
        f.executor.login_challenge_image("alice", &p.id).await,
        Err(EngineError::LoginChallengeRateLimited)
    ));
    for _ in 0..7 {
        f.executor.begin_interactive_login("alice").await.unwrap();
    }
    assert!(matches!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await,
        Err(EngineError::LoginChallengeRateLimited)
    ));
    f.executor.auth_state.lock().await.interactive.window =
        Some(Instant::now().checked_sub(Duration::from_secs(60)).unwrap());
    f.executor
        .submit_login_challenge("alice", &p.id, answer("answer", "five"))
        .await
        .unwrap();
}

#[tokio::test]
async fn multi_step_text_pin_and_custom_inputs() {
    for mode in ["multi", "plain"] {
        let f = fixture(mode).await;
        let mut p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
        if mode == "multi" {
            let old = p.id.clone();
            p = pending(
                f.executor
                    .submit_login_challenge("alice", &p.id, answer("answer", "five"))
                    .await
                    .unwrap(),
            );
            assert_ne!(old, p.id);
            assert!(
                f.executor
                    .login_challenge_image("alice", &old)
                    .await
                    .is_err()
            );
        }
        assert_eq!(p.fields[0].name, "otp");
        f.executor
            .submit_login_challenge("alice", &p.id, answer("otp", "123456"))
            .await
            .unwrap();
        assert!(f.executor.test().await.success);
    }
    for (mode, name, value) in [
        ("text", "answer", "five"),
        ("pin", "pin", "1234"),
        ("custom", "security_word", "blue"),
    ] {
        let f = fixture(mode).await;
        let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
        assert!(!p.has_image);
        if mode == "text" {
            assert_eq!(p.prompt.as_deref(), Some("What is two plus three?"));
        }
        f.executor
            .submit_login_challenge("alice", &p.id, answer(name, value))
            .await
            .unwrap();
        assert!(f.executor.test().await.success);
    }
}

#[tokio::test]
async fn credential_revocation_destroys_pending_flow() {
    let f = fixture("image").await;
    let p = pending(f.executor.begin_interactive_login("alice").await.unwrap());
    f.executor.invalidate_login().await;
    assert!(
        f.executor
            .login_challenge_image("alice", &p.id)
            .await
            .is_err()
    );
    assert!(
        f.executor
            .submit_login_challenge("alice", &p.id, answer("answer", "five"))
            .await
            .is_err()
    );
    assert!(f.executor.begin_interactive_login("alice").await.is_err());
}
