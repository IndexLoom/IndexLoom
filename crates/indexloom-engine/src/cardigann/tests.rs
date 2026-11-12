//! Tests for the parent module; extracted for easier maintenance.
use std::collections::BTreeMap;

use gtmpl::Value as TemplateValue;
use indexloom_definitions::IndexerDefinition;
use serde_yaml::Value as YamlValue;
use url::Url;

use super::{
    RenderContext, SearchParameters, all_search_paths, apply_filters, complete_result_fields,
    info_hash_from_magnet, normalize_contains_selector, normalized_result, render_search_inputs,
    render_template, retryable_search_error,
};

#[test]
fn validates_all_search_paths_and_retries_only_transient_server_failures() {
    let source: YamlValue =
        serde_yaml::from_str("paths:\n  - path: /primary\n  - path: /fallback\n")
            .expect("valid search paths");
    let paths = all_search_paths(source.as_mapping().unwrap()).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(retryable_search_error(
        &crate::EngineError::UnexpectedStatus(503)
    ));
    assert!(!retryable_search_error(
        &crate::EngineError::UnexpectedStatus(429)
    ));
    assert!(!retryable_search_error(
        &crate::EngineError::UnexpectedStatus(403)
    ));

    let broken: YamlValue = serde_yaml::from_str("paths:\n  - path: /valid\n  - invalid")
        .expect("valid YAML with invalid path shape");
    assert!(all_search_paths(broken.as_mapping().unwrap()).is_err());
}

#[test]
fn renders_structured_query_and_source_category_identifiers() {
    let parameters = SearchParameters {
        mode: Some("movie-search".to_owned()),
        category: Some("Movies/HD".to_owned()),
        genre: Some("Documentary".to_owned()),
        year: Some(2026),
        season: Some(2),
        episode: Some(4),
        imdb_id: Some("tt1234567".to_owned()),
        tmdb_id: Some("12345".to_owned()),
    };
    let context = RenderContext::new(BTreeMap::new(), BTreeMap::new()).for_search_with_parameters(
        "example",
        BTreeMap::new(),
        &parameters,
        &["0_0".to_owned(), "102".to_owned()],
    );
    let text = render_template(
            "{{ .Query.Genre }}|{{ .Query.Year }}|{{ .Query.Season }}|{{ .Query.Ep }}|{{ .Query.Type }}|{{ .Query.IMDBID }}|{{ .Query.TMDBID }}|{{ index .Categories 0 }}",
            &context,
        )
        .expect("structured search should be accessible to Cardigann templates");
    assert_eq!(
        text,
        "Documentary|2026|2|4|movie-search|tt1234567|12345|0_0"
    );
}

#[test]
fn exposes_keywords_in_root_and_cardigann_query_contexts() {
    let context = RenderContext::new(BTreeMap::new(), BTreeMap::new())
        .for_search("one piece", BTreeMap::new());
    let rendered = render_template("{{ .Keywords }}|{{ .Query.Keywords }}", &context)
        .expect("both Cardigann keyword forms should render");
    assert_eq!(rendered, "one piece|one piece");
}

#[test]
fn preserves_unicode_search_terms_through_cardigann_rendering_and_url_encoding() {
    for phrase in ["千と千尋", "Школа магии", "El viaje", "안녕하세요"] {
        let context = RenderContext::new(BTreeMap::new(), BTreeMap::new())
            .for_search(phrase, BTreeMap::new());
        let rendered = render_template("{{ .Query.Keywords }}", &context)
            .expect("multilingual keyword template should render");
        assert_eq!(rendered, phrase);
        let mut url = url::Url::parse("https://example.invalid/search").unwrap();
        url.query_pairs_mut().append_pair("q", &rendered);
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            phrase
        );
    }
}

#[test]
fn supports_common_tracker_publication_dates() {
    use super::parse_date;
    for date in [
        "2026-11-12T20:00:00Z",
        "Thu, 12 Nov 2026 20:00:00 GMT",
        "2026-11-12 20:00:00",
        "2026-11-12 20:00",
        "2026/11/12 20:00:00",
        "2026-11-12",
        "2026/11/12",
    ] {
        assert!(parse_date(date).is_some(), "should parse {date}");
    }
    assert!(parse_date("not-a-date").is_none());
}

#[test]
fn exposes_empty_values_for_unresolved_result_fields() {
    let fields: YamlValue = serde_yaml::from_str(
            "title_primary:\n  optional: true\ntitle:\n  text: '{{ or (.Result.title_primary) (.Result.title_default) }}'\ntitle_default:\n  selector: a\n",
        )
        .expect("field fixture should parse");
    let fields = fields.as_mapping().expect("fields should be a mapping");
    let resolved = BTreeMap::from([("title_default".to_owned(), "Example release".to_owned())]);
    let render_result = complete_result_fields(fields, &resolved);
    let context =
        RenderContext::new(BTreeMap::new(), BTreeMap::new()).for_search("example", render_result);
    let rendered = render_template(
        "{{ or (.Result.title_primary) (.Result.title_default) }}",
        &context,
    )
    .expect("missing optional fields should render as empty strings");
    assert_eq!(rendered, "Example release");
}

#[test]
fn renders_cardigann_conditionals() {
    let context = TemplateValue::Object(std::collections::HashMap::from([
        (
            "Keywords".to_owned(),
            TemplateValue::String("movie".to_owned()),
        ),
        ("True".to_owned(), TemplateValue::Bool(true)),
        (
            "Config".to_owned(),
            TemplateValue::Object(std::collections::HashMap::from([(
                "enabled".to_owned(),
                TemplateValue::Bool(true),
            )])),
        ),
        (
            "Result".to_owned(),
            TemplateValue::Object(BTreeMap::new().into_iter().collect()),
        ),
    ]));
    let rendered = render_template(
        "{{ if and (.Keywords) (eq .Config.enabled .True) }}search/{{ .Keywords }}{{ end }}",
        &context,
    )
    .expect("template should render");
    assert_eq!(rendered, "search/movie");
}

#[test]
fn renders_hyphenated_cardigann_configuration_names() {
    let context = TemplateValue::Object(std::collections::HashMap::from([(
        "Config".to_owned(),
        TemplateValue::Object(std::collections::HashMap::from([(
            "filter-id".to_owned(),
            TemplateValue::String("trusted".to_owned()),
        )])),
    )]));
    let rendered = render_template("{{ .Config.filter-id }}", &context)
        .expect("hyphenated config names should use the map index function");
    assert_eq!(rendered, "trusted");
}

#[test]
fn combines_path_and_search_level_inputs() {
    let search: YamlValue = serde_yaml::from_str(
        "paths:\n  - path: /\n    inputs:\n      q: '{{ .Keywords }}'\ninputs:\n  f: 0\n",
    )
    .expect("search fixture should parse");
    let search = search.as_mapping().expect("search should be a mapping");
    let path = search["paths"]
        .as_sequence()
        .and_then(|paths| paths.first())
        .and_then(YamlValue::as_mapping)
        .expect("path should be a mapping");
    let context = TemplateValue::Object(std::collections::HashMap::from([(
        "Keywords".to_owned(),
        TemplateValue::String("one piece".to_owned()),
    )]));
    let inputs = render_search_inputs(search, path, &context, false)
        .expect("both input scopes should render");
    assert_eq!(
        inputs,
        [
            ("q".to_owned(), "one piece".to_owned()),
            ("f".to_owned(), "0".to_owned())
        ]
    );
}

#[test]
fn applies_common_filters() {
    let filters: YamlValue =
        serde_yaml::from_str("- name: replace\n  args: ['-', ' ']\n- name: trim\n")
            .expect("filters should parse");
    assert_eq!(
        apply_filters(" test-release ".to_owned(), &filters).unwrap(),
        "test release"
    );
}

#[test]
fn applies_cardigann_lookaround_filters() {
    let filters: YamlValue =
        serde_yaml::from_str("- name: re_replace\n  args: ['foo(?!bar)', 'match']\n")
            .expect("filter fixture should parse");
    assert_eq!(
        apply_filters("foo baz foobar".to_owned(), &filters).unwrap(),
        "match baz foobar"
    );
}

#[test]
fn separates_contains_from_css() {
    let (selector, needles) = normalize_contains_selector("a.item:contains(\"verified\")");
    assert_eq!(selector, "a.item");
    assert_eq!(needles, ["verified"]);
}

#[test]
fn normalizes_instance_identity_and_release_links() {
    let values = BTreeMap::from([
        ("title".to_owned(), "Example release".to_owned()),
        ("details".to_owned(), "/torrent/42".to_owned()),
        ("download".to_owned(), "/download/42.torrent".to_owned()),
        ("id".to_owned(), "42".to_owned()),
        ("guid".to_owned(), "tracker-guid-42".to_owned()),
        (
            "infohash".to_owned(),
            "0123456789ABCDEF0123456789ABCDEF01234567".to_owned(),
        ),
    ]);
    let result = normalized_result(
        "example-through-proxy",
        &definition(),
        &values,
        &Url::parse("https://example.invalid/base/").unwrap(),
    )
    .expect("result should normalize");

    assert_eq!(result.indexer_instance_id, "example-through-proxy");
    assert_eq!(result.definition_id, "example");
    assert_eq!(result.source_id.as_deref(), Some("42"));
    assert_eq!(result.source_guid.as_deref(), Some("tracker-guid-42"));
    assert_eq!(
        result.info_hash.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(
        result.details_url.as_deref(),
        Some("https://example.invalid/torrent/42")
    );
    assert_eq!(
        result.download_url.as_deref(),
        Some("https://example.invalid/download/42.torrent")
    );
    assert!(result.magnet_url.is_none());
}

#[test]
fn recognizes_a_magnet_in_the_download_field() {
    let magnet = "magnet:?xt=urn%3Abtih%3AABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let values = BTreeMap::from([
        ("title".to_owned(), "Magnet release".to_owned()),
        ("download".to_owned(), magnet.to_owned()),
    ]);
    let result = normalized_result(
        "example-direct",
        &definition(),
        &values,
        &Url::parse("https://example.invalid/").unwrap(),
    )
    .expect("result should normalize");

    assert!(result.download_url.is_none());
    assert_eq!(result.magnet_url.as_deref(), Some(magnet));
    assert_eq!(
        result.info_hash.as_deref(),
        Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
    );
    assert_eq!(
        info_hash_from_magnet(magnet).as_deref(),
        Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
    );
}

#[test]
fn rejects_unsafe_links_and_uses_a_valid_magnet_fallback() {
    let fallback = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
    let values = BTreeMap::from([
        ("title".to_owned(), "Safe result".to_owned()),
        ("details".to_owned(), "javascript:alert(1)".to_owned()),
        (
            "magnet".to_owned(),
            "https://example.invalid/not-a-magnet".to_owned(),
        ),
        ("download".to_owned(), fallback.to_owned()),
    ]);
    let result = normalized_result(
        "example-direct",
        &definition(),
        &values,
        &Url::parse("https://example.invalid/").unwrap(),
    )
    .expect("valid fallback magnet should retain the result");
    assert!(result.details_url.is_none());
    assert!(result.download_url.is_none());
    assert_eq!(result.magnet_url.as_deref(), Some(fallback));

    let only_unsafe = BTreeMap::from([
        ("title".to_owned(), "Unsafe result".to_owned()),
        ("details".to_owned(), "data:text/html,unsafe".to_owned()),
        ("download".to_owned(), "file:///etc/passwd".to_owned()),
        (
            "magnet".to_owned(),
            "magnet:?dn=missing-exact-topic".to_owned(),
        ),
    ]);
    assert!(
        normalized_result(
            "example-direct",
            &definition(),
            &only_unsafe,
            &Url::parse("https://example.invalid/").unwrap(),
        )
        .is_none()
    );
}

fn definition() -> IndexerDefinition {
    serde_yaml::from_str(
        r#"
apiVersion: indexloom.io/v1alpha1
kind: Indexer
metadata:
  id: example
  name: Example
  description: Example definition
  language: en-US
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
  privacy: public
  baseUrls: [https://example.invalid/]
  implementation:
    engine: cardigann
    encoding: UTF-8
    capabilities: {}
    settings: []
    search: { paths: [{ path: /search }], rows: { selector: .result }, fields: {} }
"#,
    )
    .expect("test definition should deserialize")
}
