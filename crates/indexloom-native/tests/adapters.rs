use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use indexloom_native::*;
use serde_json::{Value, json};
use tokio::time::Instant;
use url::Url;

const FILELIST: &str = include_str!("fixtures/filelist.json");
const UNIT: &str = include_str!("fixtures/unit3d-legacy.json");
const MODERN: &str = include_str!("fixtures/unit3d-modern.json");
const AVISTA: &str = include_str!("fixtures/avistaz.json");
const AUTH: &str = include_str!("fixtures/avistaz-auth.json");
const TORRENT: &[u8] = include_bytes!("fixtures/download.torrent");

enum Step {
    Reply(HttpResponse),
    Fail(TransportError),
    Hang,
}

#[derive(Default)]
struct Mock {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<HttpRequest>>,
    starts: Mutex<Vec<Instant>>,
}

impl Mock {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            ..Self::default()
        })
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn done(&self) {
        assert!(
            self.steps.lock().unwrap().is_empty(),
            "unconsumed mock responses"
        );
    }
}

#[async_trait]
impl ValidatedTransport for Mock {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        self.starts.lock().unwrap().push(Instant::now());
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra request");
        match step {
            Step::Reply(response) => Ok(response),
            Step::Fail(error) => Err(error),
            Step::Hang => std::future::pending().await,
        }
    }
}

fn response(code: u16, content_type: &str, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status: code,
        headers: BTreeMap::from([("Content-Type".into(), content_type.into())]),
        body: body.to_vec(),
    }
}
fn reply(body: &str) -> Step {
    Step::Reply(response(
        200,
        "application/json; charset=utf-8",
        body.as_bytes(),
    ))
}
fn code(status: u16) -> Step {
    Step::Reply(response(status, "application/json", b"{}"))
}
fn torrent() -> Step {
    Step::Reply(response(200, "application/x-bittorrent", TORRENT))
}
fn filelist() -> Credentials {
    Credentials::FileList {
        username: Secret::new("user +&"),
        passkey: Secret::new("pass&+/?"),
    }
}
fn unit() -> Credentials {
    Credentials::Unit3d {
        api_token: Secret::new("api&+/?"),
        api: Unit3dApi::Legacy,
    }
}
fn modern() -> Credentials {
    Credentials::Unit3d {
        api_token: Secret::new("modern-token"),
        api: Unit3dApi::Modern,
    }
}
fn avista() -> Credentials {
    Credentials::Avistaz {
        username: Secret::new("username&+"),
        password: Secret::new("pw&+=/?"),
        pid: Secret::new("pid&+/?"),
    }
}
fn config() -> InstanceConfig {
    InstanceConfig {
        instance_id: "instance-a".into(),
        base_url: Url::parse("https://tracker.invalid/").unwrap(),
        policy: RequestPolicy::default(),
    }
}
fn adapter(credentials: Credentials, mock: &Arc<Mock>) -> NativeAdapter {
    NativeAdapter::new(config(), credentials, mock.clone()).unwrap()
}
fn pairs(request: &HttpRequest) -> BTreeMap<String, String> {
    request
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
fn value(fixture: &str) -> Value {
    serde_json::from_str(fixture).unwrap()
}

#[tokio::test(start_paused = true)]
async fn filelist_search_auth_fields_sort_and_download() {
    let mock = Mock::new(vec![reply(FILELIST), torrent()]);
    let adapter = adapter(filelist(), &mock);
    let q = NativeQuery {
        term: " Пример & test? ".into(),
        native_categories: vec![4, 21],
        season: Some(1),
        episode: Some(2),
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(page.releases.len(), 2);
    let r = &page.releases[0];
    assert_eq!(r.result.source_id.as_deref(), Some("4102"));
    assert_eq!(r.result.definition_id, "filelist");
    assert_eq!(r.result.indexer_instance_id, "instance-a");
    assert_eq!(r.result.title, "Пример.Series.S01E02.2160p.WEB-DL");
    assert_eq!(r.result.category.as_deref(), Some("TV-Series 4K"));
    assert_eq!(r.result.size, Some(15_514_501_683));
    assert_eq!((r.result.seeders, r.result.leechers), (Some(63), Some(4)));
    assert_eq!(
        r.result.published_at.unwrap().to_string(),
        "2026-07-25 21:32:22.0 +00:00:00"
    );
    assert_eq!(r.metadata.files, Some(1));
    assert_eq!(r.metadata.grabs, Some(80));
    assert!(r.metadata.internal);
    assert_eq!(r.metadata.imdb_id.as_deref(), Some("tt7654321"));
    assert_eq!(r.metadata.download_volume_factor, Some(0.0));
    assert_eq!(r.metadata.upload_volume_factor, Some(2.0));
    assert!(r.result.download_url.is_none());
    assert_eq!(adapter.download(r).await.unwrap().as_bytes(), TORRENT);
    let requests = mock.requests();
    assert_eq!(requests[0].method, Method::Get);
    assert_eq!(requests[0].url.path(), "/api.php");
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        format!("Basic {}", STANDARD.encode("user +&:pass&+/?"))
    );
    let p = pairs(&requests[0]);
    assert_eq!(p["action"], "search-torrents");
    assert_eq!(p["type"], "name");
    assert_eq!(p["query"], "Пример & test?");
    assert_eq!(p["category"], "4,21");
    assert_eq!((p["season"].as_str(), p["episode"].as_str()), ("1", "2"));
    assert_eq!(requests[1].url.path(), "/download.php");
    assert_eq!(pairs(&requests[1])["passkey"], "pass&+/?");
    assert_eq!(pairs(&requests[1])["id"], "4102");
    assert!(!requests[1].headers.contains_key("Authorization"));
    assert_eq!(requests[0].session_id, requests[1].session_id);
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn filelist_recent_imdb_limit_and_freeleech() {
    let mock = Mock::new(vec![reply(FILELIST), reply(FILELIST)]);
    let adapter = adapter(filelist(), &mock);
    let page = adapter
        .search(&NativeQuery {
            limit: 1,
            ..NativeQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page.releases.len(), 1);
    assert!(page.truncated);
    assert!(page.next_page.is_none());
    let q = NativeQuery {
        term: "ignored title".into(),
        imdb_id: Some("1234567".into()),
        freeleech_only: true,
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(page.releases.len(), 1);
    let requests = mock.requests();
    assert_eq!(pairs(&requests[0])["action"], "latest-torrents");
    assert_eq!(pairs(&requests[1])["query"], "tt1234567");
    assert_eq!(pairs(&requests[1])["type"], "imdb");
    assert_eq!(pairs(&requests[1])["freeleech"], "1");
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn unit3d_legacy_auth_pagination_fields_and_download() {
    let mock = Mock::new(vec![reply(UNIT), torrent()]);
    let adapter = adapter(unit(), &mock);
    let q = NativeQuery {
        term: "Example &".into(),
        native_categories: vec![1, 2],
        imdb_id: Some("tt1234567".into()),
        season: Some(1),
        episode: Some(2),
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(page.next_page, Some(2));
    assert_eq!(page.releases.len(), 2);
    let r = &page.releases[0];
    assert_eq!(
        r.result.info_hash.as_deref(),
        Some("a123456789012345678901234567890123456789")
    );
    assert_eq!(
        r.result.source_id.as_deref(),
        Some("A123456789012345678901234567890123456789")
    );
    assert!(page.releases[1].result.info_hash.is_none());
    assert_eq!(page.releases[1].result.size, Some(2_000_000_000));
    assert_eq!(page.releases[1].result.seeders, Some(10));
    assert_eq!(r.metadata.grabs, Some(98));
    assert_eq!(r.metadata.upload_volume_factor, Some(2.0));
    assert_eq!(
        r.result.published_at.unwrap().unix_timestamp(),
        1_793_536_496
    );
    adapter.download(r).await.unwrap();
    let requests = mock.requests();
    let p = pairs(&requests[0]);
    assert_eq!(p["api_token"], "api&+/?");
    assert_eq!(p["name"], "Example & S01E02");
    assert_eq!(p["imdb"], "1234567");
    assert_eq!(p["imdbId"], "1234567");
    assert_eq!(p["perPage"], "50");
    let cats: Vec<_> = requests[0]
        .url
        .query_pairs()
        .filter(|(k, _)| k == "categories[]")
        .map(|(_, v)| v.into_owned())
        .collect();
    assert_eq!(cats, ["1", "2"]);
    assert!(!requests[0].headers.contains_key("Authorization"));
    assert_eq!(requests[1].url.path(), "/torrent/download/5201");
    assert_eq!(pairs(&requests[1])["token"], "REDACTED");
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn unit3d_modern_bearer_percentage_and_structured_episode_filters() {
    let mock = Mock::new(vec![reply(MODERN), reply(MODERN)]);
    let adapter = adapter(modern(), &mock);
    let q = NativeQuery {
        term: "Example.Series".into(),
        season: Some(1),
        episode: Some(2),
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(page.releases[1].metadata.download_volume_factor, Some(0.5));
    assert!(page.releases[0].metadata.internal);
    let page = adapter
        .search(&NativeQuery {
            freeleech_only: true,
            ..q
        })
        .await
        .unwrap();
    assert_eq!(page.releases.len(), 1);
    let requests = mock.requests();
    assert_eq!(
        requests[0].headers["Authorization"].expose(),
        "Bearer modern-token"
    );
    assert!(!pairs(&requests[0]).contains_key("api_token"));
    assert_eq!(pairs(&requests[0])["name"], "Example Series");
    assert_eq!(pairs(&requests[0])["seasonNumber"], "1");
    assert_eq!(pairs(&requests[0])["episodeNumber"], "2");
    assert_eq!(pairs(&requests[1])["free[]"], "100");
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn unit3d_featured_and_all_observed_percentages() {
    for (percent, expected) in [
        ("0%", 1.0),
        ("25%", 0.75),
        ("50%", 0.5),
        ("75%", 0.25),
        ("100%", 0.0),
    ] {
        let mut v = value(MODERN);
        v["data"][0]["attributes"]["freeleech"] = json!(percent);
        let mock = Mock::new(vec![reply(&v.to_string())]);
        let page = adapter(modern(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap();
        assert_eq!(
            page.releases[0].metadata.download_volume_factor,
            Some(expected)
        );
    }
    let mut v = value(MODERN);
    v["data"][0]["attributes"]["featured"] = json!(true);
    v["data"][0]["attributes"]["freeleech"] = json!("0%");
    v["data"][0]["attributes"]["double_upload"] = json!(false);
    let mock = Mock::new(vec![reply(&v.to_string())]);
    let page = adapter(modern(), &mock)
        .search(&NativeQuery::default())
        .await
        .unwrap();
    assert_eq!(page.releases[0].metadata.download_volume_factor, Some(0.0));
    assert_eq!(page.releases[0].metadata.upload_volume_factor, Some(2.0));
}

#[tokio::test(start_paused = true)]
async fn avistaz_login_search_token_reuse_fields_download_and_pacing() {
    let mock = Mock::new(vec![reply(AUTH), reply(AVISTA), reply(AVISTA), torrent()]);
    let adapter = adapter(avista(), &mock);
    let q = NativeQuery {
        native_categories: vec![2],
        episode: Some(5),
        imdb_id: Some("tt1234567".into()),
        freeleech_only: true,
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(page.next_page, Some(2));
    let r = &page.releases[0];
    assert_eq!(r.result.title, "Example Series 2026 S01E05 720p WEB-DL");
    assert_eq!(r.result.source_id.as_deref(), Some("6301"));
    assert_eq!(
        r.result.info_hash.as_deref(),
        Some("b123456789012345678901234567890123456789")
    );
    assert_eq!(r.result.size, Some(935_127_615));
    assert_eq!(r.result.category.as_deref(), Some("TV-Show"));
    assert_eq!(r.result.seeders, Some(20));
    assert_eq!(r.result.leechers, Some(0));
    assert_eq!(r.metadata.audio_languages, ["Japanese"]);
    assert_eq!(r.metadata.subtitle_languages, ["English"]);
    assert_eq!(r.metadata.tmdb_id.as_deref(), Some("135144"));
    assert_eq!(
        r.result.published_at.unwrap().to_string(),
        "2026-11-13 7:39:52.0 +00:00:00"
    );
    adapter.search(&NativeQuery::default()).await.unwrap();
    adapter.download(r).await.unwrap();
    let requests = mock.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(requests[0].url.path(), "/api/v1/jackett/auth");
    let form: BTreeMap<_, _> = url::form_urlencoded::parse(&requests[0].body)
        .into_owned()
        .collect();
    assert_eq!(form["username"], "username&+");
    assert_eq!(form["password"], "pw&+=/?");
    assert_eq!(form["pid"], "pid&+/?");
    assert!(!requests[0].headers.contains_key("Authorization"));
    let p = pairs(&requests[1]);
    assert_eq!(p["type"], "2");
    assert_eq!(p["imdb"], "tt1234567");
    assert_eq!(p["search"], "E05");
    assert_eq!(p["discount[]"], "1");
    for request in &requests[1..] {
        assert_eq!(
            request.headers["Authorization"].expose(),
            "Bearer REDACTED_SESSION_TOKEN"
        );
        assert_eq!(request.session_id, requests[0].session_id);
    }
    let starts = mock.starts.lock().unwrap();
    for interval in starts.windows(2) {
        assert!(interval[1] - interval[0] >= Duration::from_millis(6100));
    }
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn avistaz_refreshes_once_for_401_or_412_and_reuses_new_token() {
    for expired in [401, 412] {
        let mock = Mock::new(vec![
            reply(AUTH),
            code(expired),
            reply("{\"token\":\"new-token\"}"),
            reply(AVISTA),
            reply(AVISTA),
        ]);
        let adapter = adapter(avista(), &mock);
        adapter.search(&NativeQuery::default()).await.unwrap();
        adapter.search(&NativeQuery::default()).await.unwrap();
        let requests = mock.requests();
        assert_eq!(requests[2].method, Method::Post);
        assert_eq!(
            requests[3].headers["Authorization"].expose(),
            "Bearer new-token"
        );
        assert_eq!(
            requests[4].headers["Authorization"].expose(),
            "Bearer new-token"
        );
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn avistaz_repeated_expiry_stops_and_next_search_reauthenticates() {
    let mock = Mock::new(vec![
        reply(AUTH),
        code(401),
        reply(AUTH),
        code(401),
        reply(AUTH),
        reply(AVISTA),
    ]);
    let adapter = adapter(avista(), &mock);
    assert!(matches!(
        adapter.search(&NativeQuery::default()).await,
        Err(Error::Authentication)
    ));
    adapter.search(&NativeQuery::default()).await.unwrap();
    assert_eq!(mock.requests().len(), 6);
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn avistaz_no_results_and_invalid_login_do_not_loop() {
    let mock = Mock::new(vec![reply(AUTH), code(404)]);
    assert!(
        adapter(avista(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap()
            .releases
            .is_empty()
    );
    mock.done();
    for step in [
        code(401),
        code(422),
        reply("{\"token\":\"\"}"),
        reply("{}"),
        reply("{\"token\":\"evil\\r\\nheader\"}"),
    ] {
        let mock = Mock::new(vec![step]);
        assert!(matches!(
            adapter(avista(), &mock)
                .search(&NativeQuery::default())
                .await,
            Err(Error::Authentication)
        ));
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn failed_post_is_never_retried() {
    for step in [code(503), Step::Fail(TransportError::Transient), Step::Hang] {
        let mock = Mock::new(vec![step]);
        assert!(
            adapter(avista(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err()
        );
        assert_eq!(mock.requests().len(), 1);
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn retryable_get_statuses_and_transport_failures_are_bounded() {
    for first in [
        code(429),
        code(502),
        code(503),
        code(504),
        Step::Fail(TransportError::Transient),
        Step::Fail(TransportError::Timeout),
        Step::Hang,
    ] {
        let mock = Mock::new(vec![first, reply(FILELIST)]);
        adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap();
        assert_eq!(mock.requests().len(), 2);
        mock.done();
    }
    let mock = Mock::new(vec![Step::Hang, Step::Hang, Step::Hang]);
    let started = Instant::now();
    assert!(matches!(
        adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await,
        Err(Error::Timeout)
    ));
    assert_eq!(mock.requests().len(), 3);
    assert!(started.elapsed() <= Duration::from_secs(92));
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn retry_after_is_honored_or_returned_to_scheduler() {
    let mut limited = response(429, "application/json", b"{}");
    limited.headers.insert("rEtRy-AfTeR".into(), "7".into());
    let mock = Mock::new(vec![Step::Reply(limited), reply(FILELIST)]);
    adapter(filelist(), &mock)
        .search(&NativeQuery::default())
        .await
        .unwrap();
    {
        let starts = mock.starts.lock().unwrap();
        assert!(starts[1] - starts[0] >= Duration::from_secs(7));
    }
    for delay in ["999999999", "Wed, 21 Oct 2026 07:28:00 GMT", "invalid"] {
        let mut limited = response(429, "application/json", b"{}");
        limited.headers.insert("Retry-After".into(), delay.into());
        let mock = Mock::new(vec![Step::Reply(limited)]);
        assert!(matches!(
            adapter(filelist(), &mock)
                .search(&NativeQuery::default())
                .await,
            Err(Error::RateLimited)
        ));
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn permanent_policy_and_http_failures_are_not_retried() {
    for (step, expected) in [
        (Step::Fail(TransportError::PolicyDenied), "egress policy"),
        (Step::Fail(TransportError::Permanent), "non-retryable"),
        (code(401), "authentication"),
        (code(403), "authentication"),
        (code(400), "HTTP status 400"),
        (code(500), "HTTP status 500"),
    ] {
        let mock = Mock::new(vec![step]);
        let error = adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected));
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn redirects_are_refused_in_auth_search_and_download() {
    let mock = Mock::new(vec![code(302)]);
    assert!(matches!(
        adapter(avista(), &mock)
            .search(&NativeQuery::default())
            .await,
        Err(Error::Redirect)
    ));
    let mock = Mock::new(vec![code(307)]);
    assert!(matches!(
        adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await,
        Err(Error::Redirect)
    ));
    let mock = Mock::new(vec![reply(FILELIST), code(302)]);
    let adapter = adapter(filelist(), &mock);
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    assert!(matches!(
        adapter.download(&page.releases[0]).await,
        Err(Error::Redirect)
    ));
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn hostile_download_details_and_continuation_urls_are_refused() {
    for path in ["download_link", "details_link"] {
        for evil in [
            "http://tracker.invalid/download",
            "https://attacker.invalid/",
            "https://tracker.invalid:444/a",
            "https://user:secret@tracker.invalid/",
            "file:///etc/passwd",
            "http://169.254.169.254/",
            "https://tracker.invalid/a#fragment",
        ] {
            let mut v = value(UNIT);
            v["data"][0]["attributes"][path] = json!(evil);
            let mock = Mock::new(vec![reply(&v.to_string())]);
            assert!(
                matches!(
                    adapter(unit(), &mock).search(&NativeQuery::default()).await,
                    Err(Error::UnsafeUrl)
                ),
                "{path}: {evil}"
            );
            mock.done();
        }
    }
    for evil in [
        "https://attacker.invalid/api/torrents/filter?page=2",
        "https://tracker.invalid/other?page=2",
        "https://tracker.invalid/api/torrents/filter?page=1",
        "https://tracker.invalid/api/torrents/filter?page=2&page=3",
    ] {
        let mut v = value(UNIT);
        v["links"]["next"] = json!(evil);
        let mock = Mock::new(vec![reply(&v.to_string())]);
        assert!(
            adapter(unit(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err()
        );
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn pagination_rebuilds_original_query_and_does_not_follow_upstream_tokens() {
    let mut v = value(UNIT);
    v["links"]["next"] = Value::Null;
    let mock = Mock::new(vec![reply(UNIT), reply(&v.to_string())]);
    let adapter = adapter(unit(), &mock);
    let mut q = NativeQuery {
        term: "original".into(),
        ..NativeQuery::default()
    };
    let page = adapter.search(&q).await.unwrap();
    assert_eq!(mock.requests().len(), 1);
    q.page = page.next_page.unwrap();
    assert!(adapter.search(&q).await.unwrap().next_page.is_none());
    let p = pairs(&mock.requests()[1]);
    assert_eq!(p["page"], "2");
    assert_eq!(p["name"], "original");
    assert_eq!(p["api_token"], "api&+/?");
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn sessions_and_download_handles_are_isolated_even_for_same_instance_id() {
    let mock = Mock::new(vec![
        reply(AUTH),
        reply(AVISTA),
        reply("{\"token\":\"other-token\"}"),
        reply(AVISTA),
    ]);
    let a = adapter(avista(), &mock);
    let b = adapter(avista(), &mock);
    let page = a.search(&NativeQuery::default()).await.unwrap();
    b.search(&NativeQuery::default()).await.unwrap();
    assert!(matches!(
        b.download(&page.releases[0]).await,
        Err(Error::ForeignRelease)
    ));
    let requests = mock.requests();
    assert_ne!(requests[0].session_id, requests[2].session_id);
    assert_eq!(
        requests[1].headers["Authorization"].expose(),
        "Bearer REDACTED_SESSION_TOKEN"
    );
    assert_eq!(
        requests[3].headers["Authorization"].expose(),
        "Bearer other-token"
    );
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn concurrent_searches_share_only_their_own_single_login() {
    let mock = Mock::new(vec![reply(AUTH), reply(AVISTA), reply(AVISTA)]);
    let adapter = adapter(avista(), &mock);
    let q = NativeQuery::default();
    let (a, b) = tokio::join!(adapter.search(&q), adapter.search(&q));
    a.unwrap();
    b.unwrap();
    assert_eq!(
        mock.requests()
            .iter()
            .filter(|r| r.method == Method::Post)
            .count(),
        1
    );
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn byte_bounds_are_passed_to_transport_and_checked_again() {
    let mut c = config();
    c.policy.max_json_bytes = 10;
    let mock = Mock::new(vec![reply(FILELIST)]);
    let adapter = NativeAdapter::new(c, filelist(), mock.clone()).unwrap();
    assert!(matches!(
        adapter.search(&NativeQuery::default()).await,
        Err(Error::BodyTooLarge)
    ));
    assert_eq!(mock.requests()[0].max_response_bytes, 10);
    assert_eq!(mock.requests()[0].timeout, Duration::from_secs(30));
    let mock = Mock::new(vec![Step::Fail(TransportError::BodyTooLarge)]);
    assert!(matches!(
        super_adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await,
        Err(Error::BodyTooLarge)
    ));
    let mut c = config();
    c.policy.max_torrent_bytes = 5;
    let mock = Mock::new(vec![reply(FILELIST), torrent()]);
    let adapter = NativeAdapter::new(c, filelist(), mock.clone()).unwrap();
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    assert!(matches!(
        adapter.download(&page.releases[0]).await,
        Err(Error::BodyTooLarge)
    ));
    assert_eq!(mock.requests()[1].max_response_bytes, 5);
    mock.done();
}

// Separate name for tests with a local `adapter` binding.
fn super_adapter(credentials: Credentials, mock: &Arc<Mock>) -> NativeAdapter {
    adapter(credentials, mock)
}

#[tokio::test(start_paused = true)]
async fn malformed_json_shapes_counts_dates_and_login_html_fail_explicitly() {
    for body in [
        "",
        "not json",
        "{",
        "{}",
        "null",
        "[{}]",
        "{\"error\":\"secret upstream diagnostic\"}",
    ] {
        let mock = Mock::new(vec![reply(body)]);
        assert!(
            adapter(filelist(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err()
        );
    }
    for (field, bad) in [
        ("size", json!(-1)),
        ("seeders", json!(4_294_967_296_u64)),
        ("upload_date", json!("yesterday")),
        ("freeleech", json!("maybe")),
        ("name", json!("")),
    ] {
        let mut v = value(FILELIST);
        v[0][field] = bad;
        let mock = Mock::new(vec![reply(&v.to_string())]);
        assert!(
            adapter(filelist(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err(),
            "field {field}"
        );
    }
    let mock = Mock::new(vec![Step::Reply(response(
        200,
        "text/html",
        b"<html>login</html>",
    ))]);
    assert!(matches!(
        adapter(unit(), &mock).search(&NativeQuery::default()).await,
        Err(Error::Parse(_))
    ));
    let mut v = value(MODERN);
    v["data"][0]["attributes"]["freeleech"] = json!("invalid%");
    let mock = Mock::new(vec![reply(&v.to_string())]);
    assert!(
        adapter(modern(), &mock)
            .search(&NativeQuery::default())
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn bom_case_insensitive_json_headers_and_empty_results_are_supported() {
    let mut r = response(200, "unused", b"\xef\xbb\xbf[]");
    r.headers = BTreeMap::from([(
        "cOnTeNt-TyPe".into(),
        "Application/JSON; charset=utf-8".into(),
    )]);
    let mock = Mock::new(vec![Step::Reply(r)]);
    assert!(
        adapter(filelist(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap()
            .releases
            .is_empty()
    );
    let mock = Mock::new(vec![reply("{\"data\":[],\"links\":{\"next\":null}}")]);
    assert!(
        adapter(unit(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap()
            .releases
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn non_torrent_download_is_rejected_even_with_a_torrent_mime_type() {
    for body in [
        b"<html>login</html>".as_slice(),
        b"{}",
        b"de",
        &TORRENT[..TORRENT.len() - 1],
    ] {
        let mock = Mock::new(vec![
            reply(FILELIST),
            Step::Reply(response(200, "application/x-bittorrent", body)),
        ]);
        let adapter = adapter(filelist(), &mock);
        let page = adapter.search(&NativeQuery::default()).await.unwrap();
        assert!(matches!(
            adapter.download(&page.releases[0]).await,
            Err(Error::Parse(_))
        ));
        mock.done();
    }
}

#[tokio::test(start_paused = true)]
async fn debug_and_errors_redact_credentials_urls_and_upstream_messages() {
    let mock = Mock::new(vec![reply(FILELIST)]);
    let adapter = adapter(filelist(), &mock);
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    let debug = format!(
        "{adapter:?} {:?} {:?} {:?}",
        page,
        mock.requests(),
        filelist()
    );
    for sensitive in [
        "pass&+/?",
        "Basic ",
        "download.php",
        "user +&",
        "retired.invalid",
    ] {
        assert!(!debug.contains(sensitive));
    }
    let mock = Mock::new(vec![reply("{\"error\":\"PASSWORD-MUST-NOT-APPEAR\"}")]);
    let error = super_adapter(filelist(), &mock)
        .search(&NativeQuery::default())
        .await
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("PASSWORD-MUST-NOT-APPEAR"));
}

#[tokio::test(start_paused = true)]
async fn validation_rejects_unsupported_queries_before_network() {
    let mock = Mock::new(vec![]);
    let adapter = adapter(filelist(), &mock);
    let invalid = [
        NativeQuery {
            page: 2,
            ..NativeQuery::default()
        },
        NativeQuery {
            limit: 501,
            ..NativeQuery::default()
        },
        NativeQuery {
            term: "a".repeat(1025),
            ..NativeQuery::default()
        },
        NativeQuery {
            imdb_id: Some("tt&bad".into()),
            ..NativeQuery::default()
        },
        NativeQuery {
            native_categories: vec![0],
            ..NativeQuery::default()
        },
        NativeQuery {
            episode: Some(1),
            ..NativeQuery::default()
        },
    ];
    for q in invalid {
        assert!(matches!(adapter.search(&q).await, Err(Error::Query(_))));
    }
    assert!(
        adapter
            .search(&NativeQuery {
                limit: 0,
                ..NativeQuery::default()
            })
            .await
            .unwrap()
            .releases
            .is_empty()
    );
    let av = super_adapter(avista(), &mock);
    for q in [
        NativeQuery {
            native_categories: vec![3],
            ..NativeQuery::default()
        },
        NativeQuery {
            limit: 51,
            ..NativeQuery::default()
        },
    ] {
        assert!(matches!(av.search(&q).await, Err(Error::Query(_))));
    }
    assert!(mock.requests().is_empty());
}

#[test]
fn invalid_config_and_secrets_are_rejected_without_io() {
    let mock = Mock::new(vec![]);
    for url in [
        "http://tracker.invalid/",
        "https://u:p@tracker.invalid/",
        "https://tracker.invalid/?pass=secret",
        "https://tracker.invalid/#fragment",
    ] {
        let mut c = config();
        c.base_url = Url::parse(url).unwrap();
        assert!(matches!(
            NativeAdapter::new(c, filelist(), mock.clone()),
            Err(Error::Configuration(_))
        ));
    }
    for credential in [
        Credentials::Unit3d {
            api_token: Secret::new(""),
            api: Unit3dApi::Modern,
        },
        Credentials::Unit3d {
            api_token: Secret::new("injected\r\nheader"),
            api: Unit3dApi::Legacy,
        },
        Credentials::FileList {
            username: Secret::new("bad:user"),
            passkey: Secret::new("pass"),
        },
    ] {
        assert!(NativeAdapter::new(config(), credential, mock.clone()).is_err());
    }
    for attempts in [0, 4] {
        let mut c = config();
        c.policy.max_attempts = attempts;
        assert!(NativeAdapter::new(c, filelist(), mock.clone()).is_err());
    }
    assert!(mock.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn reverse_proxy_base_directory_is_preserved() {
    let mock = Mock::new(vec![reply(FILELIST), torrent()]);
    let mut c = config();
    c.base_url = Url::parse("https://tracker.invalid/proxy/filelist").unwrap();
    let adapter = NativeAdapter::new(c, filelist(), mock.clone()).unwrap();
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    adapter.download(&page.releases[0]).await.unwrap();
    assert_eq!(mock.requests()[0].url.path(), "/proxy/filelist/api.php");
    assert_eq!(
        mock.requests()[1].url.path(),
        "/proxy/filelist/download.php"
    );
}

#[tokio::test(start_paused = true)]
async fn avistaz_download_renews_expired_token_and_policy_denials_stay_errors() {
    let mock = Mock::new(vec![
        reply(AUTH),
        reply(AVISTA),
        code(401),
        reply("{\"token\":\"download-token\"}"),
        torrent(),
    ]);
    let adapter = adapter(avista(), &mock);
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    let file = adapter.download(&page.releases[0]).await.unwrap();
    assert_eq!(file.into_bytes(), TORRENT);
    let requests = mock.requests();
    assert_eq!(requests[3].method, Method::Post);
    assert_eq!(requests[2].url, requests[4].url);
    assert_eq!(
        requests[4].headers["Authorization"].expose(),
        "Bearer download-token"
    );
    mock.done();

    let mock = Mock::new(vec![
        reply(FILELIST),
        Step::Fail(TransportError::PolicyDenied),
    ]);
    let adapter = super_adapter(filelist(), &mock);
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    assert!(matches!(
        adapter.download(&page.releases[0]).await,
        Err(Error::Transport(TransportError::PolicyDenied))
    ));
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn exhausted_rate_limit_is_reported_after_exact_attempt_budget() {
    for (status, expected) in [(429, "rate limited"), (503, "HTTP status 503")] {
        let mock = Mock::new(vec![code(status), code(status), code(status)]);
        let error = adapter(unit(), &mock)
            .search(&NativeQuery::default())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            if status == 429 {
                expected.to_string()
            } else {
                format!("unexpected {expected}")
            }
        );
        assert_eq!(mock.requests().len(), 3);
        mock.done();
    }
    let mut c = config();
    c.policy.max_attempts = 1;
    let mock = Mock::new(vec![code(429)]);
    let adapter = NativeAdapter::new(c, filelist(), mock.clone()).unwrap();
    assert!(matches!(
        adapter.search(&NativeQuery::default()).await,
        Err(Error::RateLimited)
    ));
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn modern_numeric_external_ids_category_ids_and_relative_signed_links() {
    let mut v = value(MODERN);
    v["links"]["next"] = Value::Null;
    let a = &mut v["data"][0]["attributes"];
    a["category"] = Value::Null;
    a["category_id"] = json!(1);
    a["imdb_id"] = json!(1_234_567);
    a["tmdb_id"] = json!(0);
    a["tvdb_id"] = json!(12345);
    a["freeleech"] = json!(75);
    a["download_link"] = json!("/torrent/download/5201?token=REDACTED");
    let mock = Mock::new(vec![reply(&v.to_string()), torrent()]);
    let adapter = adapter(modern(), &mock);
    let page = adapter.search(&NativeQuery::default()).await.unwrap();
    let r = &page.releases[0];
    assert_eq!(r.result.category.as_deref(), Some("1"));
    assert_eq!(r.metadata.imdb_id.as_deref(), Some("1234567"));
    assert!(r.metadata.tmdb_id.is_none());
    assert_eq!(r.metadata.tvdb_id.as_deref(), Some("12345"));
    assert_eq!(r.metadata.download_volume_factor, Some(0.25));
    adapter.download(r).await.unwrap();
    assert!(!mock.requests()[1].headers.contains_key("Authorization"));
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn avistaz_mixed_categories_and_local_freeleech_filtering() {
    let mut v = value(AVISTA);
    v["next_page_url"] = Value::Null;
    let mut movie = v["data"][0].clone();
    movie["id"] = json!(6302);
    movie["type"] = json!("Movie");
    movie["file_name"] = json!("Example Movie");
    movie["download_multiply"] = json!(0.5);
    v["data"].as_array_mut().unwrap().push(movie);
    let mock = Mock::new(vec![
        reply(AUTH),
        reply(&v.to_string()),
        reply(&v.to_string()),
        reply(&v.to_string()),
    ]);
    let adapter = adapter(avista(), &mock);
    let q = NativeQuery {
        native_categories: vec![1, 2],
        ..NativeQuery::default()
    };
    assert_eq!(adapter.search(&q).await.unwrap().releases.len(), 2);
    let free = NativeQuery {
        freeleech_only: true,
        ..q
    };
    assert_eq!(adapter.search(&free).await.unwrap().releases.len(), 1);
    let tv = NativeQuery {
        native_categories: vec![2],
        ..NativeQuery::default()
    };
    assert_eq!(adapter.search(&tv).await.unwrap().releases.len(), 1);
    assert_eq!(pairs(&mock.requests()[1])["type"], "0");
    mock.done();
}

#[tokio::test(start_paused = true)]
async fn malformed_family_specific_payloads_and_avistaz_links_fail_closed() {
    for body in [
        "{}",
        "{\"data\":{}}",
        "{\"data\":[{}]}",
        "{\"errors\":[\"REDACTED\"]}",
    ] {
        let mock = Mock::new(vec![reply(body)]);
        assert!(
            adapter(unit(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err()
        );
        mock.done();
        let mock = Mock::new(vec![reply(AUTH), reply(body)]);
        assert!(
            adapter(avista(), &mock)
                .search(&NativeQuery::default())
                .await
                .is_err()
        );
        mock.done();
    }
    for field in ["download", "url"] {
        let mut v = value(AVISTA);
        v["data"][0][field] = json!("https://foreign.invalid/credential-target");
        let mock = Mock::new(vec![reply(AUTH), reply(&v.to_string())]);
        assert!(matches!(
            adapter(avista(), &mock)
                .search(&NativeQuery::default())
                .await,
            Err(Error::UnsafeUrl)
        ));
        mock.done();
    }
    let mut v = value(AVISTA);
    v["next_page_url"] = json!("https://foreign.invalid/api/v1/jackett/torrents?page=2");
    let mock = Mock::new(vec![reply(AUTH), reply(&v.to_string())]);
    assert!(matches!(
        adapter(avista(), &mock)
            .search(&NativeQuery::default())
            .await,
        Err(Error::UnsafeUrl)
    ));
    mock.done();
}

#[test]
fn core_query_conversion_requires_explicit_native_category_mapping() {
    let core = indexloom_core::SearchQuery {
        term: "Example".into(),
        categories: vec!["2000".into()],
        limit: Some(20),
    };
    let q = NativeQuery::from_core(&core, vec![4, 19]);
    assert_eq!(q.native_categories, [4, 19]);
    assert_eq!(q.limit, 20);
    assert_eq!(q.term, "Example");
    assert_eq!(SUPPORTED_SITE_IDS, ["filelist", "unit3d", "avistaz"]);
}
