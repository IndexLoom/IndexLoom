//! Actual loopback HTTP tests. The test-only client maps a declared public host
//! to a local listener; the production resolver and URL policy are unchanged.
use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener},
    thread,
};

#[derive(Debug)]
struct Request {
    line: String,
    body: String,
}
fn server(
    responses: Vec<(u16, Vec<u8>, Option<String>)>,
) -> (SocketAddr, thread::JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut recorded = Vec::new();
        for (status, body, location) in responses {
            let until = Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < until,
                            "expected HTTP request never arrived"
                        );
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("listener: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                stream.read_line(&mut header).unwrap();
                if header == "\r\n" {
                    break;
                }
                assert!(!header.is_empty(), "connection closed during headers");
                if let Some((key, value)) = header.split_once(':')
                    && key.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            assert!(length < 64 * 1024);
            let mut payload = vec![0; length];
            stream.read_exact(&mut payload).unwrap();
            recorded.push(Request {
                line: line.trim().to_owned(),
                body: String::from_utf8(payload).unwrap(),
            });
            let mut stream = stream.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            )
            .unwrap();
            if let Some(location) = location {
                write!(stream, "Location: {location}\r\n").unwrap();
            }
            // Deliberately conflicting header: explicit YAML encoding has precedence.
            write!(stream, "Content-Type: text/html; charset=utf-8\r\n\r\n").unwrap();
            stream.write_all(&body).unwrap();
        }
        recorded
    });
    (address, handle)
}
fn executor(address: SocketAddr, definition: IndexerDefinition) -> CardigannExecutor {
    let mut executor = CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: "offline".into(),
        definition,
        values: BTreeMap::new(),
        secrets: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        flaresolverr: None,
    })
    .unwrap();
    let cookies = executor.engine.cookies.clone();
    executor.engine = SearchEngine {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .cookie_provider(cookies.clone())
            .resolve("example.invalid", address)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap(),
        cookies,
        browser_user_agent: std::sync::Arc::default(),
    };
    executor
}
#[tokio::test]
#[ignore = "requires loopback bind, forbidden in the managed sandbox"]
async fn executes_json_pages_without_replaying_duplicate_paths() {
    let wire = transport::WireEncoding::new("windows-1251").unwrap();
    let bodies = [
        "PREFIX{\"rows\":[{\"id\":1,\"title\":\"Привет One\"}]}",
        "PREFIX{\"rows\":[{\"id\":2,\"title\":\"Привет Two\"}]}",
    ]
    .map(|b| (200, wire.encode(b).unwrap(), None));
    let (address, server) = server(bodies.into());
    let mut definition = super::tests::definition();
    definition.spec.base_urls = vec![format!("http://example.invalid:{}/", address.port())];
    definition.spec.implementation.encoding = "windows-1251".into();
    definition.spec.implementation.search = serde_yaml::from_str(
        r"
paths:
  - {path: /search, inputs: {page: 0}, response: {type: json}}
  - {path: /search, method: post, inputs: {page: 1}, response: {type: json}}
  - {path: /search, inputs: {page: 0}, response: {type: json}}
inputs: {q: '{{ .Keywords }}', offset: '{{ .Query.Offset }}', limit: '{{ .Query.Limit }}'}
preprocessingfilters: [{name: replace, args: [PREFIX, '']}]
rows: {selector: rows}
fields:
  title: {selector: title}
  details: {text: '/details/{{ .Result.id }}'}
  id: {selector: id}
",
    )
    .unwrap();
    let batch = executor(address, definition)
        .search_page_with_parameters("Привет", 25, 50, &SearchParameters::default())
        .await
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(
        batch
            .results
            .iter()
            .map(|r| r.title.as_str())
            .collect::<Vec<_>>(),
        ["Привет One", "Привет Two"]
    );
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].line,
        "GET /search?q=%CF%F0%E8%E2%E5%F2&offset=50&limit=25&page=0 HTTP/1.1"
    );
    assert_eq!(requests[1].line, "POST /search HTTP/1.1");
    assert_eq!(
        requests[1].body,
        "q=%CF%F0%E8%E2%E5%F2&offset=50&limit=25&page=1"
    );
}
#[tokio::test]
#[ignore = "requires loopback bind, forbidden in the managed sandbox"]
async fn decodes_xml_after_same_origin_redirect_and_rejects_cross_origin_redirect() {
    let xml = "<rss><item><title>café €</title><link>/details/1</link></item></rss>";
    let wire = transport::WireEncoding::new("windows-1252").unwrap();
    let (address, server) = server(vec![
        (302, Vec::new(), Some("/feed".into())),
        (200, wire.encode(xml).unwrap(), None),
    ]);
    let mut definition = super::tests::definition();
    definition.spec.base_urls = vec![format!("http://example.invalid:{}/", address.port())];
    definition.spec.implementation.encoding = "windows-1252".into();
    definition.spec.implementation.search = serde_yaml::from_str("paths: [{path: /redirect, response: {type: xml}}]\npreprocessingfilters: [{name: trim}]\nrows: {selector: item}\nfields:\n  title: {selector: title}\n  details: {selector: link}\n").unwrap();
    let batch = executor(address, definition.clone())
        .search("café", 10)
        .await
        .unwrap();
    assert_eq!(batch.results[0].title, "café €");
    assert_eq!(server.join().unwrap().len(), 2);
    let (address, server) = self::server(vec![(
        302,
        Vec::new(),
        Some("http://127.0.0.1/private".into()),
    )]);
    definition.spec.base_urls = vec![format!("http://example.invalid:{}/", address.port())];
    assert!(matches!(
        executor(address, definition).search("café", 10).await,
        Err(EngineError::OutboundUrlRejected(_))
    ));
    server.join().unwrap();
}
#[test]
fn legacy_certificate_metadata_is_an_explicit_incompatibility() {
    let mut definition = super::tests::definition();
    definition.spec.implementation.certificates = vec!["legacy-fingerprint".into()];
    let error = CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: "offline".into(),
        definition,
        values: BTreeMap::new(),
        secrets: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        flaresolverr: None,
    })
    .unwrap_err();
    assert!(matches!(error, EngineError::CardigannIncompatible(_)));
}

#[test]
fn actual_reqwest_request_has_legacy_encoded_query_and_form_without_network() {
    for method in [Method::GET, Method::POST] {
        let mut definition = super::tests::definition();
        definition.spec.implementation.encoding = "windows-1251".into();
        let executor = CardigannExecutor::new(IndexerExecutionConfig {
            indexer_instance_id: "offline".into(),
            definition,
            values: BTreeMap::new(),
            secrets: BTreeMap::new(),
            user_agent: None,
            proxy: None,
            flaresolverr: None,
        })
        .unwrap();
        let request = executor
            .direct_request(RequestSpec {
                method: method.clone(),
                url: Url::parse("https://example.invalid/search?existing=1").unwrap(),
                headers: Vec::new(),
                inputs: vec![
                    ("q".into(), "Привет".into()),
                    ("offset".into(), "50".into()),
                ],
                raw_query: None,
            })
            .unwrap()
            .build()
            .unwrap();
        if method == Method::GET {
            assert_eq!(
                request.url().as_str(),
                "https://example.invalid/search?existing=1&q=%CF%F0%E8%E2%E5%F2&offset=50"
            );
        } else {
            assert_eq!(
                request.url().as_str(),
                "https://example.invalid/search?existing=1"
            );
            assert_eq!(
                request.headers()[reqwest::header::CONTENT_TYPE],
                "application/x-www-form-urlencoded"
            );
            assert_eq!(
                request.body().unwrap().as_bytes().unwrap(),
                b"q=%CF%F0%E8%E2%E5%F2&offset=50"
            );
        }
    }
}

#[test]
fn pem_ca_roots_extend_normal_tls_validation_per_executor() {
    let ca = include_str!("../../tests/fixtures/cardigann/test-ca.pem");
    assert_eq!(transport::certificate_roots(&[ca.into()]).unwrap().len(), 1);
    for value in [
        "sha256:legacy",
        "-----BEGIN CERTIFICATE-----\n%%%not-base64%%%\n-----END CERTIFICATE-----",
    ] {
        assert!(transport::certificate_roots(&[value.into()]).is_err());
    }
    let mut definition = super::tests::definition();
    definition.spec.implementation.certificates = vec![ca.into()];
    assert!(
        CardigannExecutor::new(IndexerExecutionConfig {
            indexer_instance_id: "offline".into(),
            definition,
            values: BTreeMap::new(),
            secrets: BTreeMap::new(),
            user_agent: None,
            proxy: None,
            flaresolverr: None
        })
        .is_ok()
    );
}
#[test]
fn shared_response_dispatch_applies_preprocessing_for_each_format() {
    for (source, body, format) in [
        (
            include_str!("../../tests/fixtures/cardigann/html.yml"),
            include_str!("../../tests/fixtures/cardigann/html.response"),
            "html",
        ),
        (
            include_str!("../../tests/fixtures/cardigann/json.yml"),
            include_str!("../../tests/fixtures/cardigann/json.response"),
            "json",
        ),
        (
            include_str!("../../tests/fixtures/cardigann/xml.yml"),
            include_str!("../../tests/fixtures/cardigann/xml.response"),
            "xml",
        ),
    ] {
        let mut definition = super::tests::definition();
        definition.spec.implementation.search = serde_yaml::from_str(source).unwrap();
        let executor = CardigannExecutor::new(IndexerExecutionConfig {
            indexer_instance_id: "offline".into(),
            definition,
            values: BTreeMap::new(),
            secrets: BTreeMap::new(),
            user_agent: None,
            proxy: None,
            flaresolverr: None,
        })
        .unwrap();
        let search = mapping(&executor.definition.spec.implementation.search).unwrap();
        let path = all_search_paths(search).unwrap()[0];
        let results = executor
            .parse_search_response(
                search,
                path,
                ExecutionResponse {
                    status: StatusCode::OK,
                    body: body.into(),
                    final_url: Url::parse("https://example.invalid/").unwrap(),
                    response_type: format.into(),
                },
                &executor.context,
                "Example Movie",
            )
            .unwrap();
        assert_eq!(results.len(), 1, "{format}");
    }
}

#[test]
fn origin_policy_errors_redact_request_secrets() {
    let definition = super::tests::definition();
    let executor = CardigannExecutor::new(IndexerExecutionConfig {
        indexer_instance_id: "offline".into(),
        definition,
        values: BTreeMap::new(),
        secrets: BTreeMap::new(),
        user_agent: None,
        proxy: None,
        flaresolverr: None,
    })
    .unwrap();
    for target in [
        "https://other.invalid/private-token?key=private-token",
        "https://user:private-token@example.invalid/",
        "ftp://example.invalid/private-token",
    ] {
        let error = executor
            .ensure_allowed(&Url::parse(target).unwrap())
            .unwrap_err();
        assert!(!error.to_string().contains("private-token"));
    }
}
