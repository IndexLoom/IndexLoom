use super::super::http::MockTransport;
use super::super::{
    DirectRequestProfile, IndexerDefinition, IndexerExecutionConfig, MAX_DIRECT_RESPONSE_BYTES,
    MAX_REDIRECTS, Origin, StatusCode, Uuid,
};
use super::*;
use reqwest::header::{HeaderMap, LOCATION};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

type MockReply = Result<(StatusCode, HeaderMap, Vec<u8>), EngineError>;
#[derive(Debug, Default)]
struct Mock {
    replies: Mutex<VecDeque<MockReply>>,
    requests: Mutex<Vec<RequestSpec>>,
}
impl Mock {
    fn new(replies: Vec<MockReply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::default(),
        })
    }
    fn requests(&self) -> Vec<RequestSpec> {
        self.requests.lock().unwrap().clone()
    }
}
impl MockTransport for Mock {
    fn send(
        &self,
        spec: RequestSpec,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = MockReply> + Send + '_>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(spec);
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected HTTP request")
        })
    }
}
#[allow(
    clippy::unnecessary_wraps,
    reason = "Mock transport uses Result for successful and failed scripted responses."
)]
fn ok(body: impl Into<Vec<u8>>) -> MockReply {
    Ok((StatusCode::OK, HeaderMap::new(), body.into()))
}
#[allow(
    clippy::unnecessary_wraps,
    reason = "Mock transport uses Result for successful and failed scripted responses."
)]
fn redirect(status: u16, target: &str) -> MockReply {
    let mut h = HeaderMap::new();
    h.insert(LOCATION, target.parse().unwrap());
    Ok((StatusCode::from_u16(status).unwrap(), h, Vec::new()))
}
#[allow(
    clippy::unnecessary_wraps,
    reason = "Mock transport uses Result for successful and failed scripted responses."
)]
fn status(value: u16) -> MockReply {
    Ok((
        StatusCode::from_u16(value).unwrap(),
        HeaderMap::new(),
        Vec::new(),
    ))
}
fn torrent() -> Vec<u8> {
    let mut bytes = b"d4:infod6:lengthi1e4:name1:x12:piece lengthi16384e6:pieces20:".to_vec();
    bytes.extend([0xff; 20]);
    bytes.extend(b"ee");
    bytes
}
fn executor(
    block: Option<&str>,
    replies: Vec<MockReply>,
) -> (CardigannExecutor, Arc<Mock>, SearchResult) {
    let mut definition: IndexerDefinition =
        serde_yaml::from_str(include_str!("fixtures/indexer.yml")).unwrap();
    definition.spec.implementation.download =
        block.map(|source| serde_yaml::from_str(source).unwrap());
    let mut executor = CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: "instance-1".to_owned(),
        definition,
        values: BTreeMap::new(),
        secrets: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        flaresolverr: None,
    })
    .unwrap();
    let mock = Mock::new(replies);
    executor.transport = Some(mock.clone());
    let release = SearchResult {
        id: Uuid::now_v7(),
        indexer_instance_id: "instance-1".to_owned(),
        definition_id: "grab-fixture".to_owned(),
        title: "Fixture release".to_owned(),
        source_id: Some("17".to_owned()),
        source_guid: None,
        info_hash: None,
        details_url: Some("https://primary.example/details?id=17".to_owned()),
        download_url: Some("https://primary.example/grab?id=17".to_owned()),
        magnet_url: None,
        category: None,
        size: None,
        seeders: None,
        leechers: None,
        published_at: None,
    };
    (executor, mock, release)
}

#[tokio::test]
async fn direct_binary_and_magnet_are_validated() {
    let (engine, mock, mut release) = executor(None, vec![ok(torrent())]);
    assert_eq!(
        engine.resolve_release(&release).await.unwrap(),
        DownloadArtifact::Torrent(torrent())
    );
    release.magnet_url = Some(format!("magnet:?xt=urn:btih:{}", "a".repeat(40)));
    assert!(matches!(
        engine.resolve_release(&release).await.unwrap(),
        DownloadArtifact::Magnet(_)
    ));
    assert_eq!(mock.requests().len(), 1);
    release.magnet_url = Some("magnet:?xt=urn:btih:garbage".to_owned());
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::InvalidMagnet)
    ));
}

#[test]
fn strict_magnet_topics() {
    for hash in [
        "A".repeat(32),
        "0123456789abcdef0123456789abcdef01234567".to_owned(),
    ] {
        assert!(validate_magnet(&format!("magnet:?xt=urn:btih:{hash}")).is_ok());
    }
    assert!(validate_magnet(&format!("magnet:?xt=urn:btmh:1220{}", "a".repeat(64))).is_ok());
    for value in [
        "magnet:?xt=urn:btih:wrong",
        "magnet:?dn=title",
        "https://example.com",
        "magnet://example.com/?xt=urn:btih:a",
        "magnet:?xt=urn:btih:00000000000000000000000000000000",
        "magnet:?xt=urn:btmh:1234",
    ] {
        assert!(validate_magnet(value).is_err(), "{value}");
    }
}

#[tokio::test]
async fn before_response_infohash_uses_download_uri_query_without_extra_get() {
    let body = format!(
        "<h1>Title &amp; subtitle</h1><div id='hash'>hash: {}</div>",
        "a".repeat(40)
    );
    let (engine, mock, release) = executor(
        Some(include_str!("fixtures/before-infohash.yml")),
        vec![ok(body)],
    );
    let DownloadArtifact::Magnet(magnet) = engine.resolve_release(&release).await.unwrap() else {
        panic!("expected magnet");
    };
    let uri = Url::parse(&magnet).unwrap();
    assert!(
        uri.query_pairs()
            .any(|(k, v)| k == "dn" && v == "Title & subtitle")
    );
    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/api/info");
    assert_eq!(requests[0].url.query(), Some("id=17"));
}

#[tokio::test]
async fn selector_fallback_handles_missing_and_invalid_torrents() {
    let html = "<a class='primary' href='/invalid'>bad</a><a class='fallback' href='/old-download/17'>good</a>";
    let (engine, mock, release) = executor(
        Some(include_str!("fixtures/selectors.yml")),
        vec![ok(html), ok("<html>not a torrent</html>"), ok(torrent())],
    );
    assert_eq!(
        engine.resolve_release(&release).await.unwrap(),
        DownloadArtifact::Torrent(torrent())
    );
    let requests = mock.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].url.path(), "/download/17");
}

#[tokio::test]
async fn follows_nested_pages_and_resolves_relative_urls() {
    let (engine, mock, release) = executor(
        Some("selectors:\n  - {selector: 'a.grab', attribute: href}"),
        vec![
            ok("<a class=grab href='/step/page'>step</a>"),
            ok("<a class=grab href='file.torrent'>final</a>"),
            ok(torrent()),
        ],
    );
    assert!(matches!(
        engine.resolve_release(&release).await.unwrap(),
        DownloadArtifact::Torrent(_)
    ));
    assert_eq!(mock.requests()[2].url.path(), "/step/file.torrent");
}

#[tokio::test]
async fn before_pathselector_post_and_usebeforeresponse() {
    let block = "before:\n  pathselector: {selector: 'form', attribute: action}\n  method: post\n  inputs: {release: '{{ .DownloadUri.Query.id }}'}\nmethod: post\nselectors:\n  - {selector: 'a.grab', attribute: href, usebeforeresponse: true}";
    let (engine, mock, release) = executor(
        Some(block),
        vec![
            ok("<form action='/confirm'></form>"),
            ok("<a class=grab href='/final'>ok</a>"),
            ok(torrent()),
        ],
    );
    engine.resolve_release(&release).await.unwrap();
    let req = mock.requests();
    assert_eq!(req.len(), 3);
    assert_eq!(req[1].method, Method::POST);
    assert_eq!(req[1].inputs, vec![("release".to_owned(), "17".to_owned())]);
    assert_eq!(req[2].method, Method::POST);
}

#[tokio::test]
async fn headers_survive_same_origin_redirects_and_307_preserves_post() {
    let block = "method: post\ninputs: {id: '17'}\nheaders: {X-Download: ['fixture']}";
    let (engine, mock, release) =
        executor(Some(block), vec![redirect(307, "/final"), ok(torrent())]);
    engine.resolve_release(&release).await.unwrap();
    let req = mock.requests();
    assert_eq!(req[1].method, Method::POST);
    assert_eq!(req[1].inputs, req[0].inputs);
    assert_eq!(req[1].headers, req[0].headers);
}

#[tokio::test]
async fn converts_303_to_get_and_accepts_valid_magnet_redirect() {
    let magnet = format!("magnet:?xt=urn:btih:{}", "a".repeat(40));
    let (engine, mock, release) = executor(
        Some("method: post"),
        vec![redirect(303, "/final"), redirect(302, &magnet)],
    );
    assert_eq!(
        engine.resolve_release(&release).await.unwrap(),
        DownloadArtifact::Magnet(magnet)
    );
    assert_eq!(mock.requests()[1].method, Method::GET);
}

#[tokio::test]
async fn unsafe_urls_stop_without_fallback_or_credential_forwarding() {
    for target in [
        "http://127.0.0.1/private",
        "http://[::1]/private",
        "http://169.254.169.254/latest",
        "https://unrelated.example/steal",
        "https://user:password@primary.example/grab",
        "file:///etc/passwd",
        "https://legacy.example/steal",
    ] {
        let (engine, mock, release) = executor(None, vec![redirect(302, target)]);
        assert!(engine.resolve_release(&release).await.is_err(), "{target}");
        assert_eq!(mock.requests().len(), 1);
    }
}

#[tokio::test]
async fn malicious_selector_is_not_hidden_by_later_selector() {
    let (engine, mock, release) = executor(
        Some(include_str!("fixtures/selectors.yml")),
        vec![ok(
            "<a class=primary href='http://127.0.0.1/secret'>bad</a><a class=fallback href='/ok'>good</a>",
        )],
    );
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::OutboundAddressRejected)
    ));
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn auth_errors_and_invalid_metadata_do_not_fail_over() {
    for error_status in [400, 401, 403, 404, 429] {
        let (engine, mock, release) = executor(None, vec![status(error_status)]);
        assert!(
            matches!(engine.resolve_release(&release).await, Err(EngineError::UnexpectedStatus(s)) if s == error_status)
        );
        assert_eq!(mock.requests().len(), 1);
    }
    let (engine, mock, release) = executor(None, vec![ok("d<html>login</html>")]);
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::InvalidTorrent)
    ));
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn mirrors_retry_then_remember_successful_legacy_origin() {
    let (engine, mock, release) = executor(None, vec![status(503), ok(torrent()), ok(torrent())]);
    engine.resolve_release(&release).await.unwrap();
    engine.resolve_release(&release).await.unwrap();
    let req = mock.requests();
    assert_eq!(req.len(), 3);
    assert_eq!(req[0].url.host_str(), Some("primary.example"));
    assert_eq!(req[1].url.host_str(), Some("legacy.example"));
    assert_eq!(req[2].url.host_str(), Some("legacy.example"));
    assert_eq!(req[1].url.query(), Some("id=17"));
}

#[tokio::test]
async fn manual_cookie_cannot_be_replayed_to_a_different_mirror() {
    let (mut engine, mock, release) = executor(None, vec![status(503)]);
    engine
        .engine
        .cookies
        .seed("session=private", &engine.base_url);
    engine.configured_cookie = true;
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::AuthenticationRejected)
    ));
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn preferred_mirror_must_be_catalog_url() {
    let (engine, mock, release) = executor(None, vec![ok(torrent())]);
    assert!(
        engine
            .set_preferred_mirror(&Url::parse("https://unrelated.example/").unwrap())
            .is_err()
    );
    engine
        .set_preferred_mirror(&Url::parse("https://legacy.example/").unwrap())
        .unwrap();
    engine.resolve_release(&release).await.unwrap();
    assert_eq!(mock.requests()[0].url.host_str(), Some("legacy.example"));
}

#[tokio::test]
async fn redirect_depth_and_response_budgets() {
    let (engine, mock, release) = executor(
        None,
        (0..=MAX_REDIRECTS)
            .map(|_| redirect(302, "/again"))
            .collect(),
    );
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::TooManyRedirects)
    ));
    assert_eq!(mock.requests().len(), MAX_REDIRECTS + 1);
    let (engine, _, release) = executor(None, vec![ok(vec![0; MAX_DIRECT_RESPONSE_BYTES + 1])]);
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::ResponseTooLarge)
    ));
    let (engine, mock, release) = executor(
        Some("selectors: [{selector: a, attribute: href}]"),
        (0..7).map(|_| ok("<a href='/again'>cycle</a>")).collect(),
    );
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::DownloadBudgetExceeded)
    ));
    assert_eq!(mock.requests().len(), 7);
}

#[tokio::test]
async fn rejects_mismatched_instance_before_network() {
    let (engine, mock, mut release) = executor(None, vec![]);
    release.indexer_instance_id = "other".to_owned();
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::ReleaseIndexerMismatch)
    ));
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn search_and_test_use_failover_too() {
    let (engine, mock, _) = executor(
        None,
        vec![
            status(502),
            ok("<div class=release><a href='/grab'>fixture</a></div>"),
            ok(""),
        ],
    );
    let batch = engine.search("fixture", 5).await.unwrap();
    assert_eq!(batch.results.len(), 1);
    assert!(engine.test().await.success);
    let req = mock.requests();
    assert_eq!(req[1].url.host_str(), Some("legacy.example"));
    assert_eq!(req[2].url.host_str(), Some("legacy.example"));
}

#[test]
fn cookies_and_browser_user_agent_stay_in_same_instance_session() {
    let (engine, _, _) = executor(None, vec![]);
    let url = engine.base_url.clone();
    engine.engine.cookies.seed("session=original", &url);
    engine.engine.import_browser_solution(
        &url,
        &crate::BrowserSolution {
            status: 200,
            user_agent: "BrowserFixture".to_owned(),
            response: String::new(),
            cookies: vec![crate::BrowserCookie {
                expires: None,
                name: "session".to_owned(),
                value: "updated".to_owned(),
                domain: Some("primary.example".to_owned()),
                path: Some("/".to_owned()),
                secure: true,
                http_only: true,
            }],
        },
    );
    assert!(
        engine
            .engine
            .cookies
            .browser_cookies(&url)
            .iter()
            .any(|cookie| cookie.name == "session" && cookie.value == "updated")
    );
    assert!(
        engine
            .engine
            .cookies
            .browser_cookies(&Url::parse("https://unrelated.example/").unwrap())
            .is_empty()
    );
    let request = engine
        .engine
        .request(
            Method::GET,
            url,
            &DirectRequestProfile {
                user_agent: None,
                cookie_header: None,
            },
        )
        .build()
        .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::USER_AGENT],
        "BrowserFixture"
    );
    let (other, _, _) = executor(None, vec![]);
    assert!(
        other
            .engine
            .cookies
            .browser_cookies(&other.base_url)
            .is_empty()
    );
}

#[tokio::test]
async fn mirror_attempts_are_bounded_and_retry_only_explicit_transient_errors() {
    let (mut engine, mock, release) = executor(None, vec![status(503), status(502), status(504)]);
    for host in ["second.example", "third.example", "fourth.example"] {
        engine
            .definition
            .spec
            .base_urls
            .push(format!("https://{host}/"));
    }
    engine.mirrors =
        Arc::new(super::super::mirrors::MirrorPool::new(&engine.definition, None).unwrap());
    engine.allowed_origins = engine
        .definition
        .spec
        .base_urls
        .iter()
        .chain(&engine.definition.spec.legacy_urls)
        .map(|v| Origin::from_url(&Url::parse(v).unwrap()))
        .collect();
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::UnexpectedStatus(504))
    ));
    assert_eq!(mock.requests().len(), 3);
    let (engine, mock, release) =
        executor(None, vec![Err(EngineError::RequestTimeout), ok(torrent())]);
    engine.resolve_release(&release).await.unwrap();
    assert_eq!(mock.requests().len(), 2);
}

#[tokio::test]
async fn each_mirror_authenticates_its_own_session_and_auth_failures_stop() {
    let (mut engine, mock, release) = executor(
        None,
        vec![
            status(503),
            {
                let mut h = HeaderMap::new();
                h.insert(
                    reqwest::header::SET_COOKIE,
                    "sid=example; Path=/; Secure".parse().unwrap(),
                );
                Ok((StatusCode::OK, h, b"login ok".to_vec()))
            },
            ok("<a class=logout>ok</a>"),
            ok(torrent()),
            ok(torrent()),
        ],
    );
    engine.definition.spec.implementation.authentication = Some(serde_yaml::from_str("path: /login\nmethod: post\ninputs: {username: '{{ .Config.username }}', password: '{{ .Config.password }}'}\ntest: {path: /account, selector: .logout}").unwrap());
    engine.context.config.insert(
        "username".to_owned(),
        TemplateValue::String("fixture-user".to_owned()),
    );
    engine.context.config.insert(
        "password".to_owned(),
        TemplateValue::String("fixture-password".to_owned()),
    );
    engine.resolve_release(&release).await.unwrap();
    engine.resolve_release(&release).await.unwrap();
    let requests = mock.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].url.host_str(), Some("primary.example"));
    assert_eq!(requests[1].url.host_str(), Some("legacy.example"));
    assert_eq!(requests[1].inputs[1].1, "fixture-password");
    assert_eq!(requests[4].url.path(), "/grab");
    let (mut engine, mock, release) =
        executor(None, vec![ok("login"), ok("<p>wrong password</p>")]);
    engine.definition.spec.implementation.authentication = Some(
        serde_yaml::from_str("path: /login\ntest: {path: /account, selector: .logout}").unwrap(),
    );
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::AuthenticationRejected)
    ));
    assert_eq!(mock.requests().len(), 2);
}

#[tokio::test]
async fn unsafe_dns_resolution_cannot_turn_into_mirror_failover() {
    // Uses only the OS localhost mapping. No external DNS or tracker traffic.
    let mut definition: IndexerDefinition =
        serde_yaml::from_str(include_str!("fixtures/indexer.yml")).unwrap();
    definition.spec.base_urls = vec!["http://localhost/".to_owned()];
    let engine = CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: "instance-1".to_owned(),
        definition,
        values: BTreeMap::new(),
        secrets: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        flaresolverr: None,
    })
    .unwrap();
    let outcome = engine.test_inner().await;
    assert!(
        matches!(outcome, Err(EngineError::OutboundAddressRejected)),
        "DNS policy must survive reqwest error wrapping: {outcome:?}"
    );
}

#[tokio::test]
async fn temporary_selector_failure_can_use_a_later_candidate_without_switching_mirrors() {
    let (engine, mock, release) = executor(
        Some(include_str!("fixtures/selectors.yml")),
        vec![
            ok("<a class=primary href='/fail'>bad</a><a class=fallback href='/good'>good</a>"),
            status(503),
            ok(torrent()),
        ],
    );
    engine.resolve_release(&release).await.unwrap();
    assert!(
        mock.requests()
            .iter()
            .all(|request| request.url.host_str() == Some("primary.example"))
    );
}

#[tokio::test]
async fn transport_authority_headers_cannot_override_url_policy() {
    let (engine, mock, release) = executor(Some("headers: {Host: unrelated.example}"), vec![]);
    assert!(matches!(
        engine.resolve_release(&release).await,
        Err(EngineError::InvalidDownloadDefinition(_))
    ));
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn nested_pages_do_not_replay_the_parent_before_side_effect() {
    let block =
        "before: {path: /confirm, method: post}\nselectors: [{selector: a, attribute: href}]";
    let (engine, mock, release) = executor(
        Some(block),
        vec![
            ok("confirmed"),
            ok("<a href='/step'>continue</a>"),
            ok("<a href='/final'>final</a>"),
            ok(torrent()),
        ],
    );
    engine.resolve_release(&release).await.unwrap();
    assert_eq!(
        mock.requests()
            .iter()
            .filter(|request| request.url.path() == "/confirm")
            .count(),
        1
    );
}
