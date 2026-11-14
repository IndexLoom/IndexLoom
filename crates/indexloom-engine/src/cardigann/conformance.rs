//! Offline behavior fixtures, independently authored after observing both upstreams.
use super::tests::definition;
use super::*;
fn yaml(source: &str) -> YamlValue {
    serde_yaml::from_str(source).unwrap()
}
fn context() -> RenderContext {
    RenderContext::new(BTreeMap::new(), BTreeMap::new())
}
fn run(input: &str, pipeline: &str) -> Result<String, EngineError> {
    transform::apply(input.into(), &yaml(pipeline), &TemplateValue::Nil, "UTF-8")
}
const HTML_YAML: &str = include_str!("../../tests/fixtures/cardigann/html.yml");
const HTML_BODY: &str = include_str!("../../tests/fixtures/cardigann/html.response");
const JSON_YAML: &str = include_str!("../../tests/fixtures/cardigann/json.yml");
const JSON_BODY: &str = include_str!("../../tests/fixtures/cardigann/json.response");
const XML_YAML: &str = include_str!("../../tests/fixtures/cardigann/xml.yml");
const XML_BODY: &str = include_str!("../../tests/fixtures/cardigann/xml.response");
#[test]
fn complete_string_filter_inventory() {
    for case in yaml(include_str!("../../tests/fixtures/cardigann/filters.yml"))
        .as_array()
        .unwrap()
    {
        let actual = transform::apply(
            case["input"].as_str().unwrap().into(),
            &case["filters"],
            &TemplateValue::Nil,
            "UTF-8",
        )
        .unwrap();
        assert_eq!(
            actual,
            case["expected"].as_str().unwrap(),
            "{:?}",
            case["filters"]
        );
    }
}
#[test]
fn malformed_filters_are_structured_errors_without_input() {
    for pipeline in [
        "{}",
        "- bogus",
        "- args: x",
        "- name: unknown",
        "- {name: split, args: [',', nonsense]}",
        "- {name: split, args: [',', 4]}",
        "- {name: regexp}",
        "- {name: replace, args: [x]}",
        "- {name: diacritics, args: delete}",
        "- {name: base64decode}",
        "- {name: urldecode}",
        "- {name: dateparse, args: yyyy}",
        "- {name: append, args: {x: y}}",
        "- {name: tolower, args: unexpected}",
    ] {
        let error = run("sensitive-%invalid", pipeline).unwrap_err();
        assert!(
            matches!(error, EngineError::CardigannFilter { .. }),
            "{pipeline}: {error:?}"
        );
        assert!(!error.to_string().contains("sensitive"));
    }
    assert!(run("text", "- {name: re_replace, args: ['(', x]}").is_err());
    assert!(run("{}", "- {name: jsonjoinarray, args: ['$[?', ',']}").is_err());
}
#[test]
fn dates_cover_dotnet_go_relative_and_fuzzy_formats() {
    let now = parse_date("2026-11-14T10:00:00Z").unwrap();
    for (input, format, expected) in [
        (
            "2026-11-14 12:30:45 +03:00",
            "yyyy-MM-dd HH:mm:ss zzz",
            "2026-11-14T09:30:45Z",
        ),
        (
            "2026-11-1412:30:45 +03:00",
            "yyyy-MM-ddHH:mm:ss zzz",
            "2026-11-14T09:30:45Z",
        ),
        (
            "14 November 2026 09:30:45 +00:00",
            "d MMMM yyyy HH:mm:ss zzz",
            "2026-11-14T09:30:45Z",
        ),
        (
            "Nov 14 2026 09:30 am",
            "MMM d yyyy hh:mm tt",
            "2026-11-14T09:30:00Z",
        ),
        (
            "2026-11-14T09:30:45+00:00",
            "2006-01-02T15:04:05-07:00",
            "2026-11-14T09:30:45Z",
        ),
        ("14 Nov 26", "dd MMM yy", "2026-11-14T00:00:00Z"),
        ("12-31 08:00", "MM-dd HH:mm", "2025-12-31T08:00:00Z"),
        ("09:30 +00:00", "HH:mm zzz", "2026-11-14T09:30:00Z"),
    ] {
        assert_eq!(
            dates::layout(input, format, now),
            parse_date(expected),
            "{input} / {format}"
        );
        assert_eq!(
            dates::filter("dateparse", input, &[format.into()], now).unwrap(),
            expected
        );
        assert_eq!(
            dates::filter("timeparse", input, &[format.into()], now).unwrap(),
            expected
        );
    }
    for name in ["timeago", "reltime"] {
        assert_eq!(
            dates::filter(name, "1 day, 2 hours and 3 minutes ago", &[], now).unwrap(),
            "2026-11-13T07:57:00Z"
        );
    }
    for (input, expected) in [
        ("Yesterday at 09:30", "2026-11-13T09:30:00Z"),
        ("today, 09:30", "2026-11-14T09:30:00Z"),
        ("Friday at 09:30", "2026-11-13T09:30:00Z"),
        ("2 hours ago", "2026-11-14T08:00:00Z"),
        ("now", "2026-11-14T10:00:00Z"),
        ("Nov 14 09:30", "2026-11-14T09:30:00Z"),
    ] {
        assert_eq!(
            dates::fuzzy(input, None, now),
            parse_date(expected),
            "{input}"
        );
    }
    assert!(dates::relative("2 fortnights", now).is_none());
    assert!(dates::relative("junk 2 days", now).is_none());
    assert!(dates::layout("2026-02-30", "yyyy-MM-dd", now).is_none());
}
#[test]
fn jsonpath_selects_nested_arrays_quoted_keys_unions_slices_and_predicates() {
    let value = serde_json::json!({"a.b":{"items":[{"id":1,"free":true,"tags":["one"]},{"id":2,"free":false},{"id":3,"free":true}]}, "other":{"id":4}});
    let check = |path: &str, expected: JsonValue| {
        let actual = jsonpath::select(&value, path)
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(JsonValue::Array(actual), expected, "{path}");
    };
    check("$['a.b'].items[0,2].id", serde_json::json!([1, 3]));
    check("$['a.b'].items[*].id", serde_json::json!([1, 2, 3]));
    check("$['a.b'].items[-1].id", serde_json::json!([3]));
    check("$['a.b'].items[0:3:2].id", serde_json::json!([1, 3]));
    check("$['a.b'].items[::-1].id", serde_json::json!([3, 2, 1]));
    check(
        "$['a.b'].items[?(@.free == true)].id",
        serde_json::json!([1, 3]),
    );
    check(
        "$['a.b'].items[*]:has(tags):not(free:contains(false)).id",
        serde_json::json!([1]),
    );
    let mut recursive = jsonpath::select(&value, "$..id")
        .unwrap()
        .into_iter()
        .map(|v| v.as_i64().unwrap())
        .collect::<Vec<_>>();
    recursive.sort_unstable();
    assert_eq!(recursive, [1, 2, 3, 4]);
    for path in [
        "$.a[",
        "$..",
        "$[?(@.free > 1)]",
        "$['unclosed]",
        "$[::0]",
        "$[a]",
        "$.",
        "$:unknown(x)",
    ] {
        assert!(jsonpath::select(&value, path).is_err(), "{path}");
    }
    assert!(jsonpath::select(&value, "missing").unwrap().is_empty());
    let nested = serde_json::json!({"rows":[[{"id":1}],[{"id":2}]]});
    assert_eq!(jsonpath::select(&nested, "rows[*][*].id").unwrap().len(), 2);
}
#[test]
fn css_contains_respects_ancestors_nested_negation_and_siblings() {
    let doc = Html::parse_document(
        "<section><div class='yes'><b>verified</b><a href='/one'>one</a></div><div><a href='/two'>verified</a></div><p>tail</p></section>",
    );
    for (query, expected) in [
        ("div:contains('verified') > a", 2),
        ("div:has(b:contains('verified')) > a", 1),
        ("div:not(:has(b:contains('verified'))) > a", 1),
        ("div.yes + div > a[href='/two']", 1),
        ("div.yes ~ p", 1),
        ("a:contains('one'), p:contains('tail')", 2),
    ] {
        assert_eq!(
            selectors::Css::parse(query)
                .unwrap()
                .select(doc.root_element(), true)
                .unwrap()
                .len(),
            expected,
            "{query}"
        );
    }
    for query in ["a[", "a:unknown(x)", "div:contains('x'", "div >>> a"] {
        assert!(selectors::Css::parse(query).is_err(), "{query}");
    }
}
fn parse_fixture(source: &str, body: &str, format: &str) -> Vec<SearchResult> {
    let mut definition = definition();
    definition.spec.implementation.search = yaml(source);
    let search = mapping(&definition.spec.implementation.search).unwrap();
    let context = context();
    let render = context.for_search("Example Movie", BTreeMap::new());
    let body = search
        .get(key("preprocessingfilters"))
        .map_or_else(
            || Ok(body.into()),
            |f| transform::apply(body.into(), f, &render, "UTF-8"),
        )
        .unwrap();
    let url = Url::parse("https://example.invalid/").unwrap();
    match format {
        "json" => parse_json_results(
            "instance",
            &definition,
            search,
            &body,
            &context,
            "Example Movie",
            &url,
        ),
        "xml" => extract_html::parse_document_results(
            "instance",
            &definition,
            search,
            xml::document(&body).unwrap(),
            &context,
            "Example Movie",
            &url,
        ),
        _ => parse_html_results(
            "instance",
            &definition,
            search,
            &body,
            &context,
            "Example Movie",
            &url,
        ),
    }
    .unwrap()
}
#[test]
fn realistic_html_json_and_xml_definitions_extract_complete_releases() {
    let html = parse_fixture(HTML_YAML, HTML_BODY, "html");
    assert_eq!(html.len(), 1);
    assert_eq!(html[0].title, "Example Movie");
    assert_eq!(html[0].size, Some(1_610_612_736));
    assert_eq!(html[0].published_at, parse_date("2026-11-14T09:30:45Z"));
    let json = parse_fixture(JSON_YAML, JSON_BODY, "json");
    assert_eq!(json.len(), 1);
    assert_eq!(json[0].title, "Example Movie & More");
    assert_eq!(
        json[0].details_url.as_deref(),
        Some("https://example.invalid/details/42")
    );
    let xml = parse_fixture(XML_YAML, XML_BODY, "xml");
    assert_eq!(xml.len(), 1);
    assert_eq!(xml[0].title, "Example & Movie");
    assert_eq!(xml[0].seeders, Some(12));
    assert_eq!(xml[0].published_at, parse_date("2026-11-14T09:30:45Z"));
}
#[test]
fn xml_is_case_sensitive_and_rejects_dtds_entities_and_broken_trees() {
    for body in [
        "<rss><item></rss>",
        "<!DOCTYPE rss [<!ENTITY bad SYSTEM 'file:///etc/passwd'>]><rss>&bad;</rss>",
        "<rss>&unknown;</rss>",
        "<rss/ ><other/>",
        "<rss>",
    ] {
        assert!(xml::document(body).is_err(), "{body}");
    }
    let doc = xml::document("<root><Item ID='X'/><item id='y'/></root>").unwrap();
    assert_eq!(
        selectors::Css::parse("Item[ID='X']")
            .unwrap()
            .select(doc.root_element(), true)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        selectors::Css::parse("item[ID='X']")
            .unwrap()
            .select(doc.root_element(), true)
            .unwrap()
            .len(),
        0
    );
}
#[test]
fn legacy_encodings_are_strict_and_apply_to_requests_and_filters() {
    for (label, text, escaped) in [
        ("windows-1251", "Привет", "%CF%F0%E8%E2%E5%F2"),
        ("windows-1252", "café €", "caf%E9+%80"),
        ("ISO-8859-1", "café \u{0080}", "caf%E9+%80"),
    ] {
        let wire = transport::WireEncoding::new(label).unwrap();
        assert_eq!(wire.decode(&wire.encode(text).unwrap()).unwrap(), text);
        assert_eq!(wire.escape(text).unwrap(), escaped);
        let filters = yaml("- name: urlencode\n- name: urldecode");
        assert_eq!(
            transform::apply(text.into(), &filters, &TemplateValue::Nil, label).unwrap(),
            text
        );
    }
    assert!(transport::WireEncoding::new("unknown-encoding").is_err());
    assert!(
        transport::WireEncoding::new("ISO-8859-1")
            .unwrap()
            .encode("日本")
            .is_err()
    );
    assert!(
        transport::WireEncoding::new("UTF-8")
            .unwrap()
            .decode(&[255])
            .is_err()
    );
}
#[tokio::test(start_paused = true)]
async fn request_delay_serializes_concurrent_starts_and_is_cancellation_safe() {
    let gate = std::sync::Arc::new(transport::RequestGate::new(Some(2.0)).unwrap());
    let started = tokio::time::Instant::now();
    gate.wait().await;
    let g = gate.clone();
    let one = tokio::spawn(async move {
        g.wait().await;
        tokio::time::Instant::now()
    });
    let g = gate.clone();
    let two = tokio::spawn(async move {
        g.wait().await;
        tokio::time::Instant::now()
    });
    assert_eq!(one.await.unwrap() - started, Duration::from_secs(2));
    assert_eq!(two.await.unwrap() - started, Duration::from_secs(4));
    let g = gate.clone();
    let cancelled = tokio::spawn(async move { g.wait().await });
    tokio::task::yield_now().await;
    cancelled.abort();
    gate.wait().await;
    assert_eq!(
        tokio::time::Instant::now() - started,
        Duration::from_secs(6)
    );
    for delay in [f64::NAN, f64::INFINITY, -1.0, 1e100] {
        assert!(transport::RequestGate::new(Some(delay)).is_err());
    }
}
#[test]
fn templates_apply_to_filter_arguments_and_request_pagination() {
    let mut render = context().for_search("hello", BTreeMap::from([("id".into(), "42".into())]));
    set_pagination(&mut render, 50, 100);
    assert_eq!(
        render_template("{{ .Query.Limit }}|{{ .Query.Offset }}", &render).unwrap(),
        "50|100"
    );
    assert_eq!(
        transform::apply(
            "item".into(),
            &yaml("- {name: append, args: '/{{ .Result.id }}'}"),
            &render,
            "UTF-8"
        )
        .unwrap(),
        "item/42"
    );
    assert_eq!(
        render_template("{{ re_replace \"ab\" \"(a)(b)\" \"$2$1\" }}", &render).unwrap(),
        "ba"
    );
    let search = yaml(
        "inputs: {q: inherited, page: 0}\npaths: [{path: /, inputs: {page: '{{ .Query.Offset }}'}}, {path: /, inheritinputs: false, inputs: {page: 2}}]",
    );
    let search = mapping(&search).unwrap();
    let paths = all_search_paths(search).unwrap();
    assert_eq!(
        render_search_inputs(search, paths[0], &render, false).unwrap(),
        [
            ("q".into(), "inherited".into()),
            ("page".into(), "100".into())
        ]
    );
    assert_eq!(
        render_search_inputs(search, paths[1], &render, false).unwrap(),
        [("page".into(), "2".into())]
    );
}
#[test]
fn row_filter_inventory_and_andmatch_character_limit() {
    let filters = rows::RowFilters::parse(
        mapping(&yaml(
            "filters: [{name: andmatch, args: 7}, {name: strdump}]",
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(filters.allows("EXAMPLE.other", "Example absent"));
    assert!(!filters.allows("Absent", "Example absent"));
    assert!(rows::RowFilters::parse(mapping(&yaml("filters: [{name: nope}]")).unwrap()).is_err());
}

#[test]
fn field_removals_affect_cases_without_mutating_other_fields() {
    let doc = Html::parse_document(
        "<div class='row'><a href='/details/1'>Main<span class='badge'>VIP</span></a></div>",
    );
    let row = selectors::Css::parse("div.row")
        .unwrap()
        .select(doc.root_element(), true)
        .unwrap()[0];
    let spec = yaml("selector: a\nremove: span\ncase: {'span': wrong, '*': clean}");
    assert_eq!(
        extract_html::field(row, mapping(&spec).unwrap(), &TemplateValue::Nil, "UTF-8")
            .unwrap()
            .as_deref(),
        Some("clean")
    );
    let spec = yaml("selector: a");
    assert_eq!(
        extract_html::field(row, mapping(&spec).unwrap(), &TemplateValue::Nil, "UTF-8")
            .unwrap()
            .as_deref(),
        Some("MainVIP")
    );
    let spec = yaml("selector: div.row\nattribute: class");
    assert_eq!(
        extract_html::field(row, mapping(&spec).unwrap(), &TemplateValue::Nil, "UTF-8")
            .unwrap()
            .as_deref(),
        Some("row")
    );
}
#[test]
fn html_after_merges_rows_and_dateheaders_walk_preceding_sections() {
    let source = "paths: [{path: /}]\nrows:\n  selector: tr.release\n  after: 1\n  dateheaders: {selector: h3, filters: [{name: dateparse, args: yyyy-MM-dd}]}\nfields:\n  title: {selector: a}\n  details: {selector: a, attribute: href}\n  size: {selector: .size}\n";
    let body = "<h3>2026-11-14</h3><section><table><tr class='release'><td><a href='/details/1'>Example Movie</a></td></tr><tr class='release'><td class='size'>2 GiB</td></tr></table></section>";
    let releases = parse_fixture(source, body, "html");
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].size, Some(2_147_483_648));
    assert_eq!(releases[0].published_at, parse_date("2026-11-14"));
}
#[test]
fn json_count_missing_attributes_and_optional_defaults_are_distinct() {
    let source = "paths: [{path: /}]\nrows:\n  selector: data\n  attribute: records\n  multiple: true\n  missingAttributeEqualsNoResults: true\n  count: {selector: count}\nfields:\n  title: {selector: name}\n  details: {selector: url}\n  seeders: {selector: seeds, optional: true, default: '3'}\n  description: {selector: absent}\n";
    assert!(parse_fixture(source, r#"{"count":0,"data":[]}"#, "json").is_empty());
    let result = parse_fixture(
        source,
        r#"{"count":2,"data":[{"records":[{"name":"Example Movie","url":"/1"}]},{}]}"#,
        "json",
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].seeders, Some(3));
    assert!(parse_fixture(source, r#"{"count":0}"#, "json").is_empty());
    assert!(parse_fixture(source, r#"{"count":5}"#, "json").is_empty());
}
#[test]
fn helper_cycles_are_explicit_and_append_modifiers_run_once() {
    let fields =
        yaml("title: {text: '{{ .Result.helper }}'}\nhelper: {text: '{{ .Result.title }}'}");
    let result = extract::fields(
        mapping(&fields).unwrap(),
        &context(),
        "x",
        |spec, render| {
            Ok(Some(render_template(
                spec["text"].as_str().unwrap(),
                render,
            )?))
        },
    );
    assert!(matches!(result, Err(EngineError::CardigannIncompatible(_))));
    let fields = yaml(
        "title: {text: base}\n\"title|append\": {text: '-{{ .Result.helper }}'}\nhelper: {text: suffix}",
    );
    let result = extract::fields(
        mapping(&fields).unwrap(),
        &context(),
        "x",
        |spec, render| {
            Ok(Some(render_template(
                spec["text"].as_str().unwrap(),
                render,
            )?))
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(result["title"], "base-suffix");
}
#[test]
fn complexity_budgets_reject_oversized_paths_patterns_and_pipelines() {
    assert!(filters::cardigann_regex(&"a".repeat(16_385)).is_err());
    assert!(jsonpath::select(&serde_json::json!({}), &"key.".repeat(129)).is_err());
    let wide = JsonValue::Array(vec![JsonValue::Null; 100_001]);
    assert!(jsonpath::select(&wide, "$[*]").is_err());
    let filters = YamlValue::Array(vec![yaml("{name: trim}"); 129]);
    assert!(transform::apply("value".into(), &filters, &TemplateValue::Nil, "UTF-8").is_err());
    let lookbehind = run(
        "prefixX",
        "- {name: re_replace, args: ['(?<=(prefix))X', '$1']}",
    )
    .unwrap();
    assert_eq!(lookbehind, "prefixprefix");
}
#[test]
fn id_search_bypasses_andmatch_only_when_advertised() {
    let mut definition = definition();
    let params = SearchParameters {
        imdb_id: Some("tt42".into()),
        ..SearchParameters::default()
    };
    assert!(!templates::supports_id_search(&definition, &params));
    definition.spec.implementation.capabilities = yaml("modes: {movie-search: [q, imdbid]}");
    assert!(templates::supports_id_search(&definition, &params));
    let paths = yaml("paths: [{path: /a, categories: ['1']}, {path: /b, categories: ['!', '1']}] ");
    let paths = all_search_paths(mapping(&paths).unwrap()).unwrap();
    assert!(path_matches_categories(paths[0], &["1".into()]).unwrap());
    assert!(!path_matches_categories(paths[1], &["1".into()]).unwrap());
    assert!(path_matches_categories(paths[1], &["2".into()]).unwrap());
}

#[test]
fn legacy_template_values_are_encoded_before_path_and_raw_query_construction() {
    let render = context().for_search("Привет & мир", BTreeMap::new());
    assert_eq!(
        templates::render_encoded_template(
            "/search/{{ if .Keywords }}{{ .Keywords }}{{ end }}?page=1",
            &render,
            "windows-1251",
            true
        )
        .unwrap(),
        "/search/%CF%F0%E8%E2%E5%F2%20%26%20%EC%E8%F0?page=1"
    );
    assert_eq!(
        templates::render_encoded_template(
            "q={{ .Keywords }}&page=1",
            &render,
            "windows-1251",
            false
        )
        .unwrap(),
        "q=%CF%F0%E8%E2%E5%F2+%26+%EC%E8%F0&page=1"
    );
    let render = context().for_search_with_parameters(
        "x",
        BTreeMap::new(),
        &SearchParameters::default(),
        &["1".into(), "2".into()],
    );
    assert_eq!(
        templates::render_encoded_template(
            "{{ range .Categories }}cat[]={{ . }}&{{ end }}q={{ .Keywords }}",
            &render,
            "UTF-8",
            false
        )
        .unwrap(),
        "cat[]=1&cat[]=2&q=x"
    );
}

#[test]
fn scope_and_relative_contains_match_the_selected_row() {
    let doc = Html::parse_document(
        "<section><div class='row'><span><a>release</a></span><b>one</b><i><b>nested</b></i></div><p>tail</p></section>",
    );
    let row = selectors::Css::parse("div.row")
        .unwrap()
        .select(doc.root_element(), true)
        .unwrap()[0];
    assert_eq!(
        selectors::Css::parse(":scope > span > a")
            .unwrap()
            .select(row, true)
            .unwrap()
            .len(),
        1
    );
    for (query, count) in [
        ("div:has(> b:contains('one'))", 1),
        ("div:has(> b:contains('nested'))", 0),
        ("div:has(+ p:contains('tail'))", 1),
    ] {
        assert_eq!(
            selectors::Css::parse(query)
                .unwrap()
                .select(doc.root_element(), true)
                .unwrap()
                .len(),
            count,
            "{query}"
        );
    }
}

#[test]
fn declared_encoding_applies_only_to_escaped_bytes_in_urldecode() {
    assert_eq!(
        transform::apply(
            "café+%80".into(),
            &yaml("- name: urldecode"),
            &TemplateValue::Nil,
            "windows-1252"
        )
        .unwrap(),
        "café €"
    );
    for label in ["UTF-16LE", "UTF-16BE"] {
        let wire = transport::WireEncoding::new(label).unwrap();
        assert_eq!(wire.decode(&wire.encode("日本").unwrap()).unwrap(), "日本");
    }
    assert_eq!(
        templates::render_encoded_template(
            "/{{/* user's comment */}}{{ .Keywords }}",
            &context().for_search("a b", BTreeMap::new()),
            "UTF-8",
            true
        )
        .unwrap(),
        "/a%20b"
    );
}
